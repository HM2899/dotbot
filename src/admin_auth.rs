//! Single administrator, Argon2id passwords and revocable, persistent browser sessions.
use std::{collections::{HashMap, VecDeque}, io::Write, path::{Path, PathBuf}, sync::{Arc, Mutex}, time::{Duration, Instant}};
use anyhow::{Context, Result, ensure};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const SESSION_DAYS: i64 = 7;
const REMEMBER_DAYS: i64 = 30;

#[derive(Clone, Serialize, Deserialize)]
struct Credentials {
    version: u32,
    #[serde(default = "default_username")]
    username: String,
    #[serde(default)]
    setup_required: bool,
    password: String,
    #[serde(default)]
    sessions: HashMap<String, i64>,
}

pub struct AdminAuth {
    path: PathBuf,
    state: Mutex<Credentials>,
    attempts: Mutex<VecDeque<Instant>>,
    workers: Arc<tokio::sync::Semaphore>,
    setup_lock: tokio::sync::Mutex<()>,
}

#[derive(Debug)]
pub enum LoginError { Invalid, Validation(&'static str), Limited, Unavailable }

fn default_username() -> String { "admin".into() }
fn username_ok(username: &str) -> bool {
    (1..=64).contains(&username.chars().count()) && username.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-'))
}
const USERNAME_RULE: &str = "用户名须为 1–64 个字符，可用中文、字母、数字、点、下划线和短横线。";

fn password_ok(password: &str) -> Result<()> {
    ensure!(password.chars().count() >= 6 && password.len() <= 256, "密码须至少 6 个字符，且不超过 256 字节。");
    ensure!(!password.trim().is_empty(), "密码不能全为空白。");
    Ok(())
}

fn hash_password(password: &str) -> Result<String> {
    password_ok(password)?;
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|_| anyhow::anyhow!("无法生成密码盐"))?;
    let salt = SaltString::encode_b64(&salt).map_err(|_| anyhow::anyhow!("无法生成密码盐"))?;
    Argon2::default().hash_password(password.as_bytes(), &salt).map(|v| v.to_string()).map_err(|_| anyhow::anyhow!("无法保存密码"))
}

fn verify(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|hash| Argon2::default().verify_password(password.as_bytes(), &hash).is_ok())
}

fn fingerprint(token: &str) -> String { hex::encode(Sha256::digest(token.as_bytes())) }
fn now() -> i64 { chrono::Utc::now().timestamp() }

fn auth_dir(home: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let dir = home.join("web-auth");
    std::fs::create_dir_all(&dir)?;
    ensure!(std::fs::symlink_metadata(&dir)?.file_type().is_dir(), "登录目录不能是符号链接。");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

fn save(path: &Path, credentials: &Credentials) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let temporary = path.with_file_name(format!(".admin-{}", uuid::Uuid::new_v4().simple()));
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(&temporary)?;
        file.write_all(&serde_json::to_vec(credentials)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        std::fs::File::open(path.parent().unwrap())?.sync_all()?;
        Ok(())
    })();
    let _ = std::fs::remove_file(temporary);
    result
}

impl AdminAuth {
    pub fn open(home: &Path) -> Result<Option<Arc<Self>>> {
        use std::os::unix::fs::PermissionsExt;
        let path = auth_dir(home)?.join("admin.json");
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        ensure!(meta.file_type().is_file() && meta.len() <= 64 * 1024, "管理员登录文件无效。");
        let state: Credentials = serde_json::from_slice(&crate::sqlite_storage::read(&path)?).context("管理员登录文件损坏；请恢复备份或使用 admin-password 重置")?;
        ensure!(state.version == 1 && state.password.starts_with("$argon2id$") && PasswordHash::new(&state.password).is_ok(), "管理员密码格式无效。");
        ensure!(username_ok(&state.username), "管理员用户名无效。");
        ensure!(state.sessions.len() <= 32 && state.sessions.keys().all(|key| key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit())), "管理员会话数据无效。");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Some(Arc::new(Self { path, state: Mutex::new(state), attempts: Mutex::default(), workers: Arc::new(tokio::sync::Semaphore::new(2)), setup_lock: tokio::sync::Mutex::new(()) })))
    }

    /// Caller holds the server lock. Never replaces existing or damaged credentials.
    pub fn initialize(home: &Path) -> Result<(Arc<Self>, Option<String>)> {
        if let Some(auth) = Self::open(home)? { return Ok((auth, None)); }
        let password = format!("dotbot-{}", &super::token()[..32]);
        let state = Credentials { version: 1, username: default_username(), setup_required: true, password: hash_password(&password)?, sessions: HashMap::new() };
        save(&auth_dir(home)?.join("admin.json"), &state)?;
        Ok((Self::open(home)?.context("无法创建初始管理员账号")?, Some(password)))
    }

    /// Caller holds the same-home server lock. There is no public registration/reset API.
    pub fn set_password(home: &Path, password: &str, username: Option<&str>) -> Result<()> {
        let username = match username {
            Some(username) => username.to_string(),
            None => Self::open(home).ok().flatten().map(|auth| auth.username()).unwrap_or_else(default_username),
        };
        ensure!(username_ok(&username), USERNAME_RULE);
        let state = Credentials { version: 1, username, setup_required: false, password: hash_password(password)?, sessions: HashMap::new() };
        save(&auth_dir(home)?.join("admin.json"), &state)
    }

    pub fn username(&self) -> String { self.state.lock().unwrap().username.clone() }
    pub fn setup_required(&self) -> bool { self.state.lock().unwrap().setup_required }

    /// Persist the profile before enabling application access. Failure keeps the setup gate closed.
    pub async fn complete_setup<F, Fut>(&self, token: String, username: String, password: String, save_profile: F) -> Result<(), LoginError>
    where F: FnOnce() -> Fut, Fut: std::future::Future<Output = Result<()>> {
        let _guard = self.setup_lock.lock().await;
        let permit = self.admit()?;
        if !self.setup_required() || !self.valid(&token) { return Err(LoginError::Invalid); }
        if !username_ok(&username) { return Err(LoginError::Validation(USERNAME_RULE)); }
        if password_ok(&password).is_err() { return Err(LoginError::Validation("新密码须至少 6 个字符，且不超过 256 字节。")); }
        let expected = self.state.lock().unwrap().password.clone();
        let hash = expected.clone();
        let replacement = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if verify(&password, &hash) { return Err(LoginError::Validation("请设置一个与初始密码不同的新密码。")); }
            hash_password(&password).map_err(|_| LoginError::Unavailable)
        }).await.map_err(|_| LoginError::Unavailable)??;
        save_profile().await.map_err(|_| LoginError::Unavailable)?;
        let mut state = self.state.lock().unwrap();
        if !state.setup_required || state.password != expected || !state.sessions.get(&fingerprint(&token)).is_some_and(|until| *until > now()) { return Err(LoginError::Invalid); }
        let next = Credentials { version: 1, username, setup_required: false, password: replacement, sessions: HashMap::new() };
        save(&self.path, &next).map_err(|_| LoginError::Unavailable)?;
        *state = next;
        Ok(())
    }

    fn admit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, LoginError> {
        let mut attempts = self.attempts.lock().unwrap();
        let time = Instant::now();
        while attempts.front().is_some_and(|t| time.duration_since(*t) >= Duration::from_secs(60)) { attempts.pop_front(); }
        if attempts.len() >= 20 { return Err(LoginError::Limited); }
        let permit = self.workers.clone().try_acquire_owned().map_err(|_| LoginError::Limited)?;
        attempts.push_back(time);
        Ok(permit)
    }

    pub fn valid(&self, token: &str) -> bool {
        token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()) && self.state.lock().unwrap().sessions.get(&fingerprint(token)).is_some_and(|until| *until > now())
    }

    pub async fn login(&self, username: String, password: String, remember: bool) -> Result<(String, i64), LoginError> {
        let permit = self.admit()?;
        if password.len() > 256 { return Err(LoginError::Invalid); }
        let (expected_username, expected) = { let state = self.state.lock().unwrap(); (state.username.clone(), state.password.clone()) };
        let hash = expected.clone();
        let account = expected_username.clone();
        let verified = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let checked = verify(&password, &hash);
            checked && username == account
        }).await.map_err(|_| LoginError::Unavailable)?;
        if !verified { return Err(LoginError::Invalid); }
        let mut state = self.state.lock().unwrap();
        if state.password != expected || state.username != expected_username { return Err(LoginError::Invalid); }
        let mut next = state.clone();
        next.sessions.retain(|_, until| *until > now());
        if next.sessions.len() >= 32 {
            let oldest = next.sessions.iter().min_by_key(|(_, until)| *until).map(|(id, _)| id.clone()).unwrap();
            next.sessions.remove(&oldest);
        }
        let seconds = if next.setup_required { 15 * 60 } else { (if remember { REMEMBER_DAYS } else { SESSION_DAYS }) * 86400 };
        let token = super::token();
        next.sessions.insert(fingerprint(&token), now() + seconds);
        save(&self.path, &next).map_err(|_| LoginError::Unavailable)?;
        *state = next;
        Ok((token, seconds))
    }

    pub fn logout(&self, token: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let mut next = state.clone();
        next.sessions.remove(&fingerprint(token));
        save(&self.path, &next)?;
        *state = next;
        Ok(())
    }

    pub async fn change_credentials(&self, token: String, old: String, username: Option<String>, new: Option<String>) -> Result<(), LoginError> {
        let permit = self.admit()?;
        if !self.valid(&token) || old.len() > 256 { return Err(LoginError::Invalid); }
        let (expected_username, expected) = { let state = self.state.lock().unwrap(); (state.username.clone(), state.password.clone()) };
        let username = username.unwrap_or_else(|| expected_username.clone());
        if !username_ok(&username) { return Err(LoginError::Validation(USERNAME_RULE)); }
        if new.as_ref().is_some_and(|new| password_ok(new).is_err()) { return Err(LoginError::Validation("新密码须至少 6 个字符，且不超过 256 字节。")); }
        if username == expected_username && new.is_none() { return Err(LoginError::Validation("请修改用户名或填写新密码。")); }
        let hash = expected.clone();
        let replacement = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if !verify(&old, &hash) { return Err(LoginError::Invalid); }
            match new { Some(new) => hash_password(&new).map_err(|_| LoginError::Unavailable), None => Ok(hash) }
        }).await.map_err(|_| LoginError::Unavailable)??;
        let mut state = self.state.lock().unwrap();
        if state.password != expected || state.username != expected_username || !state.sessions.get(&fingerprint(&token)).is_some_and(|until| *until > now()) { return Err(LoginError::Invalid); }
        let next = Credentials { version: 1, username, setup_required: false, password: replacement, sessions: HashMap::new() };
        save(&self.path, &next).map_err(|_| LoginError::Unavailable)?;
        *state = next;
        Ok(())
    }

    pub fn revoke_all(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let mut next = state.clone(); next.sessions.clear();
        save(&self.path, &next)?; *state = next; Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn initial_password_setup_gate_persistence_failure_and_revocation() {
        let home = tempfile::tempdir().unwrap();
        let (auth, password) = AdminAuth::initialize(home.path()).unwrap();
        let password = password.unwrap();
        assert!(auth.setup_required());
        assert!(!std::fs::read_to_string(&auth.path).unwrap().contains(&password));
        let (token, seconds) = auth.login("admin".into(), password.clone(), true).await.unwrap();
        assert_eq!(seconds, 900);
        let (reopened, generated) = AdminAuth::initialize(home.path()).unwrap();
        assert!(generated.is_none()); assert!(reopened.valid(&token)); assert!(reopened.setup_required());
        assert!(matches!(auth.complete_setup(token.clone(), "owner".into(), password.clone(), || async { panic!("same password must not save profile") }).await, Err(LoginError::Validation(_))));
        assert!(auth.complete_setup(token.clone(), "owner".into(), "new-private-password-123".into(), || async { anyhow::bail!("disk unavailable") }).await.is_err());
        assert!(auth.setup_required()); assert!(auth.valid(&token));
        assert!(auth.complete_setup("0".repeat(64), "owner".into(), "new-private-password-123".into(), || async { panic!("unauthorized profile save") }).await.is_err());
        auth.complete_setup(token.clone(), "owner".into(), "new-private-password-123".into(), || async { Ok(()) }).await.unwrap();
        assert!(!auth.setup_required()); assert!(!auth.valid(&token));
        let (reopened, generated) = AdminAuth::initialize(home.path()).unwrap();
        assert!(generated.is_none()); assert!(!reopened.setup_required());
        assert!(reopened.login("admin".into(), password, false).await.is_err());
        let (token, _) = reopened.login("owner".into(), "new-private-password-123".into(), false).await.unwrap();
        assert!(reopened.complete_setup(token, "attacker".into(), "other-private-password-123".into(), || async { panic!("completed setup must not save profile") }).await.is_err());
    }
    #[tokio::test]
    async fn legacy_account_renames_password_changes_and_cli_recovery() {
        let home = tempfile::tempdir().unwrap();
        AdminAuth::set_password(home.path(), "safe-password-1234", None).unwrap();
        let path = home.path().join("web-auth/admin.json");
        let mut legacy: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        legacy.as_object_mut().unwrap().remove("username");
        legacy.as_object_mut().unwrap().remove("setup_required");
        std::fs::write(&path, legacy.to_string()).unwrap();
        let auth = AdminAuth::open(home.path()).unwrap().unwrap();
        assert_eq!(auth.username(), "admin");
        assert!(!auth.setup_required());
        let (token, _) = auth.login("admin".into(), "safe-password-1234".into(), false).await.unwrap();
        assert!(matches!(auth.change_credentials(token.clone(), "safe-password-1234".into(), Some("bad name".into()), None).await, Err(LoginError::Validation(_))));
        assert!(auth.change_credentials(token.clone(), "wrong".into(), Some("负责人.dotbot".into()), None).await.is_err());
        assert!(auth.valid(&token));
        auth.change_credentials(token.clone(), "safe-password-1234".into(), Some("负责人.dotbot".into()), None).await.unwrap();
        assert!(!auth.valid(&token));
        assert!(auth.login("admin".into(), "safe-password-1234".into(), false).await.is_err());
        let (token, _) = auth.login("负责人.dotbot".into(), "safe-password-1234".into(), false).await.unwrap();
        auth.change_credentials(token.clone(), "safe-password-1234".into(), None, Some("changed-password-1234".into())).await.unwrap();
        assert!(!auth.valid(&token));
        let reopened = AdminAuth::open(home.path()).unwrap().unwrap();
        assert_eq!(reopened.username(), "负责人.dotbot");
        assert!(reopened.login("负责人.dotbot".into(), "changed-password-1234".into(), false).await.is_ok());
        AdminAuth::set_password(home.path(), "recovery-password-1234", None).unwrap();
        assert_eq!(AdminAuth::open(home.path()).unwrap().unwrap().username(), "负责人.dotbot");
        std::fs::write(&path, "corrupt").unwrap();
        AdminAuth::set_password(home.path(), "recovery-password-1234", Some("owner")).unwrap();
        assert_eq!(AdminAuth::open(home.path()).unwrap().unwrap().username(), "owner");
    }
    #[tokio::test]
    async fn password_sessions_restart_logout_and_reset() {
        let home = tempfile::tempdir().unwrap();
        assert!(AdminAuth::open(home.path()).unwrap().is_none());
        assert!(AdminAuth::set_password(home.path(), "short", None).is_err());
        assert!(password_ok("abc123").is_ok());
        AdminAuth::set_password(home.path(), "a-long-password-123", None).unwrap();
        let auth = AdminAuth::open(home.path()).unwrap().unwrap();
        assert!(matches!(auth.login("other".into(), "a-long-password-123".into(), false).await, Err(LoginError::Invalid)));
        let (first, seconds) = auth.login("admin".into(), "a-long-password-123".into(), false).await.unwrap();
        assert_eq!(seconds, 7*86400); assert!(auth.valid(&first));
        assert!(!std::fs::read_to_string(&auth.path).unwrap().contains(&first));
        let auth = AdminAuth::open(home.path()).unwrap().unwrap();
        assert!(auth.valid(&first));
        let (second, _) = auth.login("admin".into(), "a-long-password-123".into(), true).await.unwrap();
        auth.logout(&first).unwrap(); assert!(!auth.valid(&first)); assert!(auth.valid(&second));
        assert!(auth.change_credentials(second.clone(), "wrong".into(), None, Some("changed-password-123".into())).await.is_err());
        auth.change_credentials(second.clone(), "a-long-password-123".into(), None, Some("changed-password-123".into())).await.unwrap();
        assert!(!auth.valid(&second));
        assert!(auth.login("admin".into(), "a-long-password-123".into(), false).await.is_err());
        let (third, _) = auth.login("admin".into(), "changed-password-123".into(), false).await.unwrap();
        auth.revoke_all().unwrap(); assert!(!auth.valid(&third));
        let (fourth, _) = auth.login("admin".into(), "changed-password-123".into(), false).await.unwrap();
        { let mut state = auth.state.lock().unwrap(); state.sessions.insert(fingerprint(&fourth), now()-1); }
        assert!(!auth.valid(&fourth));
    }
    #[tokio::test]
    async fn rejects_corruption_symlinks_and_limits_attempts() {
        use std::os::unix::{fs::{symlink, PermissionsExt}};
        let home = tempfile::tempdir().unwrap();
        AdminAuth::set_password(home.path(), "safe-password-1234", None).unwrap();
        let auth = AdminAuth::open(home.path()).unwrap().unwrap();
        assert_eq!(std::fs::metadata(&auth.path).unwrap().permissions().mode()&0o777, 0o600);
        for _ in 0..20 { drop(auth.admit().unwrap()); }
        assert!(matches!(auth.admit(), Err(LoginError::Limited)));
        std::fs::write(&auth.path, "bad").unwrap(); assert!(AdminAuth::open(home.path()).is_err());
        std::fs::remove_file(&auth.path).unwrap();
        symlink(home.path().join("missing"), &auth.path).unwrap(); assert!(AdminAuth::open(home.path()).is_err());
    }
}
