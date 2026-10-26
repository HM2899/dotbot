//! Team account authorization. Credentials stay on the server; tools use bounded,
//! relative API paths and the account's explicitly granted permissions.
use crate::{durable, store::now_ms};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoints {
    pub authorize: String,
    pub token: String,
    pub api: String,
    pub identity: String,
    #[serde(default)]
    pub revoke: String,
    /// Further API bases a multi-service account (Google) may call by full URL.
    #[serde(default)]
    pub bases: Vec<String>,
}

/// A provider's OAuth app, registered once so each account signs in with one click.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct App {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
}

const GOOGLE_SCOPE: &str = "https://www.googleapis.com/auth/";
const GOOGLE_IDENTITY: [&str; 3] = ["openid", "email", "profile"];
/// The services one Google sign-in covers: read and write scopes, and the API bases a Bot
/// may call for each. Photos only reaches media this app created or the user picks: Google
/// retired full-library access for third-party apps in 2025.
fn google_services() -> Value {
    let scopes = |list: &[&str]| list.iter().map(|x| format!("{GOOGLE_SCOPE}{x}")).collect::<Vec<_>>();
    let service = |id: &str, name: &str, read: &[&str], write: &[&str], apis: &[&str]| {
        json!({"id":id,"name":name,"read":scopes(read),"write":scopes(write),"apis":apis})
    };
    json!([
        service("gmail", "Gmail", &["gmail.readonly"], &["gmail.modify", "gmail.send"], &["https://gmail.googleapis.com/gmail/v1", "https://www.googleapis.com/gmail/v1", "https://gmail.googleapis.com/upload/gmail/v1"]),
        service("calendar", "Google Calendar", &["calendar.readonly"], &["calendar.events"], &["https://www.googleapis.com/calendar/v3"]),
        service("drive", "Google Drive", &["drive.readonly"], &["drive.file"], &["https://www.googleapis.com/drive/v3", "https://www.googleapis.com/upload/drive/v3"]),
        service("docs", "Google Docs", &["documents.readonly"], &["documents"], &["https://docs.googleapis.com/v1"]),
        service("sheets", "Google Sheets", &["spreadsheets.readonly"], &["spreadsheets"], &["https://sheets.googleapis.com/v4"]),
        service("contacts", "Google Contacts", &["contacts.readonly"], &["contacts"], &["https://people.googleapis.com/v1"]),
        service("tasks", "Google Tasks", &["tasks.readonly"], &["tasks"], &["https://tasks.googleapis.com/tasks/v1", "https://www.googleapis.com/tasks/v1"]),
        service("photos", "Google Photos", &["photoslibrary.readonly.appcreateddata", "photospicker.mediaitems.readonly"], &["photoslibrary.appendonly"], &["https://photoslibrary.googleapis.com/v1", "https://photospicker.googleapis.com/v1"]),
    ])
}
fn strings(value: &Value) -> Vec<String> {
    value.as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect()
}

pub fn catalog() -> Value {
    let services = google_services();
    let all = |key: &str| services.as_array().unwrap().iter().flat_map(|s| strings(&s[key])).collect::<Vec<_>>();
    let mut read: Vec<String> = GOOGLE_IDENTITY.iter().map(|s| s.to_string()).collect();
    read.extend(all("read"));
    // Sign-in starts with read access to everything except Photos; each service is adjustable afterwards.
    let mut first: Vec<String> = GOOGLE_IDENTITY.iter().map(|s| s.to_string()).collect();
    first.extend(services.as_array().unwrap().iter().filter(|s| s["id"] != "photos").flat_map(|s| strings(&s["read"])));
    json!([
        {"id":"google","name":"Google","scopes":read,"writeScopes":all("write"),"defaultScopes":first,"services":services,"docs":"https://console.cloud.google.com/apis/credentials"},
        {"id":"github","name":"GitHub","scopes":["read:user"],"writeScopes":["repo","admin:repo_hook"],"defaultScopes":["read:user"],"docs":"https://github.com/settings/developers"},
        {"id":"microsoft","name":"Microsoft 365","scopes":["offline_access","User.Read","Mail.Read","Calendars.Read","Files.Read"],"writeScopes":["Mail.Send","Calendars.ReadWrite","Files.ReadWrite"],"defaultScopes":["offline_access","User.Read","Mail.Read","Calendars.Read","Files.Read"],"docs":"https://portal.azure.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade"}
    ])
}

/// Single-service Google accounts saved before the combined Google sign-in. They keep
/// working and can be paused, revoked or removed, but new ones use "google".
fn legacy_catalog() -> Value {
    json!([
        {"id":"gmail","name":"Gmail","scopes":["https://www.googleapis.com/auth/gmail.readonly"],"writeScopes":["https://www.googleapis.com/auth/gmail.modify","https://www.googleapis.com/auth/gmail.send"],"docs":"https://console.cloud.google.com/apis/credentials"},
        {"id":"calendar","name":"Google Calendar","scopes":["https://www.googleapis.com/auth/calendar.readonly"],"writeScopes":["https://www.googleapis.com/auth/calendar.events"],"docs":"https://console.cloud.google.com/apis/credentials"},
        {"id":"drive","name":"Google Drive","scopes":["https://www.googleapis.com/auth/drive.readonly"],"writeScopes":["https://www.googleapis.com/auth/drive.file"],"docs":"https://console.cloud.google.com/apis/credentials"}
    ])
}

fn catalog_row(provider: &str) -> Option<Value> {
    [catalog(), legacy_catalog()].into_iter().flat_map(|c| c.as_array().unwrap().clone()).find(|p| p["id"] == provider)
}
/// Whether accounts of `provider` can still be authorized and used.
fn supported(provider: &str) -> bool {
    catalog_row(provider).is_some()
}
/// Whether `url` is `base` or below it.
fn under(url: &reqwest::Url, base: &str) -> bool {
    let Ok(base) = reqwest::Url::parse(base) else { return false };
    let root = base.path().trim_end_matches('/');
    url.origin() == base.origin() && (url.path() == root || url.path().starts_with(&format!("{root}/")))
}
/// Google scopes come back from the token service in their long form.
fn scope_granted(granted: &[String], scope: &str) -> bool {
    let long = match scope { "email" => format!("{GOOGLE_SCOPE}userinfo.email"), "profile" => format!("{GOOGLE_SCOPE}userinfo.profile"), other => other.to_string() };
    granted.iter().any(|g| g == scope || *g == long)
}
/// For Google, each request must fall under a service this account enables: any of its
/// scopes for reads, a write scope otherwise. Turning a service down applies at once,
/// without signing in again.
fn service_allowed(account: &Account, url: &reqwest::Url, method: &str) -> Result<()> {
    if account.provider != "google" || under(url, "https://www.googleapis.com/oauth2/v3/userinfo") {
        return Ok(());
    }
    let services = google_services();
    let service = services.as_array().unwrap().iter()
        .find(|s| strings(&s["apis"]).iter().any(|base| under(url, base)))
        .ok_or_else(|| anyhow!("This Google API is not one of the services this account covers."))?;
    let has = |key: &str| strings(&service[key]).iter().any(|s| account.scopes.contains(s));
    let name = service["name"].as_str().unwrap_or("This service");
    // Creating a Photos picker session is a POST but only lets the user pick what to share.
    let reading = method == "GET" || url.host_str() == Some("photospicker.googleapis.com");
    if reading && !has("read") && !has("write") {
        bail!("{name} is turned off for this account; enable it in Settings.");
    }
    if !reading && !has("write") {
        bail!("{name} is read-only for this account; allow writes in Settings.");
    }
    Ok(())
}

pub fn defaults(provider: &str) -> Result<Endpoints> {
    let google = |api: &str, identity: &str| Endpoints {
        authorize: "https://accounts.google.com/o/oauth2/v2/auth".into(),
        token: "https://oauth2.googleapis.com/token".into(),
        api: api.into(),
        identity: identity.into(),
        revoke: "https://oauth2.googleapis.com/revoke".into(),
        bases: Vec::new(),
    };
    Ok(match provider {
        "google" => Endpoints {
            bases: google_services().as_array().unwrap().iter().flat_map(|s| strings(&s["apis"])).collect(),
            ..google("https://www.googleapis.com", "/oauth2/v3/userinfo")
        },
        "calendar" => google(
            "https://www.googleapis.com/calendar/v3",
            "/users/me/calendarList?maxResults=1",
        ),
        "gmail" => google("https://gmail.googleapis.com/gmail/v1", "/users/me/profile"),
        "drive" => google("https://www.googleapis.com/drive/v3", "/about?fields=user"),
        "github" => Endpoints {
            authorize: "https://github.com/login/oauth/authorize".into(),
            token: "https://github.com/login/oauth/access_token".into(),
            api: "https://api.github.com".into(),
            identity: "/user".into(),
            revoke: String::new(),
            bases: Vec::new(),
        },
        "microsoft" => Endpoints {
            authorize: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize".into(),
            token: "https://login.microsoftonline.com/common/oauth2/v2.0/token".into(),
            api: "https://graph.microsoft.com/v1.0".into(),
            identity: "/me".into(),
            revoke: String::new(),
            bases: Vec::new(),
        },
        _ => bail!("Choose an account from the connection catalog."),
    })
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    pub scopes: Vec<String>,
    #[serde(default)]
    pub granted_scopes: Vec<String>,
    #[serde(default)]
    pub writable: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub endpoints: Endpoints,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub expires: Option<i64>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub identity: String,
    #[serde(default)]
    pub checked: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub generation: u64,
}
fn yes() -> bool {
    true
}
impl Account {
    pub fn view(&self) -> Value {
        let services: Vec<Value> = if self.provider == "google" {
            google_services().as_array().unwrap().iter().map(|s| {
                let has = |key: &str| strings(&s[key]).iter().any(|x| self.scopes.contains(x));
                json!({"id":s["id"],"name":s["name"],"access":if has("write") { "write" } else if has("read") { "read" } else { "off" }})
            }).collect()
        } else { Vec::new() };
        json!({"services":services,"id":self.id,"name":self.name,"provider":self.provider,"clientId":self.client_id,"hasClientSecret":!self.client_secret.is_empty(),"scopes":self.scopes,"grantedScopes":self.granted_scopes,"writable":self.writable,"enabled":self.enabled,"status":self.status,"identity":self.identity,"checked":self.checked,"error":self.error,"expires":self.expires})
    }
}

struct Pending {
    account: String,
    verifier: String,
    redirect: String,
    expires: i64,
    generation: u64,
}
pub struct Accounts {
    apps_path: PathBuf,
    apps: Mutex<std::collections::BTreeMap<String, App>>,
    path: PathBuf,
    list: Mutex<Vec<Account>>,
    pending: Mutex<HashMap<String, Pending>>,
    gate: tokio::sync::Mutex<()>,
}

fn endpoint(raw: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw)?;
    let local = url.host_str().is_some_and(|h| {
        h == "localhost"
            || h.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if (!matches!(url.scheme(), "https") && !(url.scheme() == "http" && local))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        bail!("Account endpoints require HTTPS (HTTP is allowed only for loopback testing).");
    }
    Ok(url)
}
fn api_url(account: &Account, path: &str) -> Result<reqwest::Url> {
    if path.starts_with("https://") && !account.endpoints.bases.is_empty() {
        // Multi-service accounts call one of their listed API bases by full URL.
        if path.len() > 4096 || path.contains(['\\', '#']) {
            bail!("Use one of this account's API URLs.");
        }
        let url = endpoint(path)?;
        if !account.endpoints.bases.iter().any(|base| under(&url, base)) {
            bail!("The API URL is not one of this account's services.");
        }
        return Ok(url);
    }
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.contains(['\\', '#'])
        || path.len() > 4096
    {
        bail!("Use a relative API path starting with one /.");
    }
    let base = endpoint(&account.endpoints.api)?;
    let url = endpoint(&format!(
        "{}{path}",
        account.endpoints.api.trim_end_matches('/')
    ))?;
    if url.origin() != base.origin()
        || !(url.path() == base.path().trim_end_matches('/')
            || url
                .path()
                .starts_with(&format!("{}/", base.path().trim_end_matches('/'))))
    {
        bail!("The API path leaves the connected service.");
    }
    // Never expose the token through endpoints that redirect or echo credentials.
    Ok(url)
}
fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("dotbot/0.1")
        .build()?)
}
async fn response_json(mut response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    if !status.is_success() {
        bail!("Account API returned HTTP {status}; check permissions or reconnect.");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Account response interrupted"))?
    {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            bail!("Account response exceeds 1 MiB; request fewer fields or a smaller page.");
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        return Ok(json!({"ok":true}));
    }
    serde_json::from_slice(&bytes).context("The account API did not return JSON")
}

impl Accounts {
    pub fn open(home: &Path) -> Result<Self> {
        let path = home.join("accounts.json");
        let list = match crate::sqlite_storage::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("reading accounts.json")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        let apps_path = home.join("oauth-apps.json");
        let apps = match crate::sqlite_storage::read(&apps_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("reading oauth-apps.json")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Default::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            apps_path,
            apps: Mutex::new(apps),
            path,
            list: Mutex::new(list),
            pending: Mutex::new(HashMap::new()),
            gate: Default::default(),
        })
    }
    /// The provider's sign-in app: saved in Settings, or DOTBOT_<PROVIDER>_CLIENT_ID and
    /// DOTBOT_<PROVIDER>_CLIENT_SECRET when the server starts.
    fn app(&self, provider: &str) -> Option<(App, bool)> {
        if let Some(app) = self.apps.lock().unwrap().get(provider).filter(|a| !a.client_id.is_empty()) {
            return Some((app.clone(), false));
        }
        let key = provider.to_uppercase();
        let client_id = crate::branding::env(format!("DOTBOT_{key}_CLIENT_ID")).ok().filter(|v| !v.trim().is_empty())?;
        let client_secret = crate::branding::env(format!("DOTBOT_{key}_CLIENT_SECRET")).unwrap_or_default();
        Some((App { client_id: client_id.trim().into(), client_secret }, true))
    }
    pub fn apps_view(&self) -> Value {
        catalog().as_array().unwrap().iter().filter_map(|p| p["id"].as_str()).map(|id| {
            let app = self.app(id);
            (id.to_string(), json!({
                "configured": app.is_some(),
                "clientId": app.as_ref().map(|(a, _)| a.client_id.clone()).unwrap_or_default(),
                "hasClientSecret": app.as_ref().is_some_and(|(a, _)| !a.client_secret.is_empty()),
                "fromEnvironment": app.as_ref().is_some_and(|(_, env)| *env),
            }))
        }).collect::<serde_json::Map<_, _>>().into()
    }
    pub fn save_app(&self, params: &Value) -> Result<Value> {
        let provider = params["provider"].as_str().unwrap_or_default();
        if !catalog().as_array().unwrap().iter().any(|p| p["id"] == provider) {
            bail!("Choose an account from the connection catalog.");
        }
        let client_id = params["clientId"].as_str().unwrap_or_default().trim().to_string();
        if client_id.is_empty() {
            bail!("Enter the Client ID of the OAuth app.");
        }
        let mut apps = self.apps.lock().unwrap();
        let mut next = apps.clone();
        let kept = next.get(provider).filter(|a| a.client_id == client_id).map(|a| a.client_secret.clone()).unwrap_or_default();
        let client_secret = params["clientSecret"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from).unwrap_or(kept);
        next.insert(provider.into(), App { client_id, client_secret });
        durable::save(&self.apps_path, &next)?;
        *apps = next;
        drop(apps);
        Ok(self.apps_view())
    }
    /// One-click sign-in: a new account with the provider's starting permissions, and the
    /// authorization address to open right away.
    pub fn connect(&self, provider: &str, origin: &str) -> Result<Value> {
        let row = catalog().as_array().unwrap().iter().find(|p| p["id"] == provider).cloned()
            .ok_or_else(|| anyhow!("Choose an account from the connection catalog."))?;
        let (app, _) = self.app(provider).ok_or_else(|| anyhow!("Set up the sign-in app first."))?;
        let saved = self.save(&json!({"provider":provider,"name":row["name"],"clientId":app.client_id,"clientSecret":app.client_secret,"scopes":row["defaultScopes"],"writable":false}))?;
        let id = saved["id"].as_str().unwrap_or_default().to_string();
        match self.authorize(&id, origin) {
            Ok(mut value) => { value["id"] = json!(id); Ok(value) }
            Err(error) => {
                let mut list = self.list.lock().unwrap();
                let next: Vec<Account> = list.iter().filter(|a| a.id != id).cloned().collect();
                durable::save(&self.path, &next)?;
                *list = next;
                Err(error)
            }
        }
    }
    pub fn view(&self) -> Vec<Value> {
        self.list
            .lock()
            .unwrap()
            .iter()
            .map(Account::view)
            .collect()
    }
    pub fn get(&self, id: &str) -> Result<Account> {
        self.list
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .ok_or_else(|| anyhow!("Unknown account"))
    }
    fn commit(&self, account: Account) -> Result<()> {
        let mut list = self.list.lock().unwrap();
        let mut next = list.clone();
        let slot = next
            .iter_mut()
            .find(|a| a.id == account.id)
            .ok_or_else(|| anyhow!("Unknown account"))?;
        if slot.generation != account.generation {
            bail!("The account changed; retry with its current settings.");
        }
        *slot = account;
        durable::save(&self.path, &next)?;
        *list = next;
        Ok(())
    }
    pub fn save(&self, params: &Value) -> Result<Value> {
        let provider = params["provider"].as_str().unwrap_or_default();
        let default = defaults(provider)?;
        let existing = params["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(|id| self.get(id))
            .transpose()?;
        if existing.as_ref().is_some_and(|a| a.provider != provider) {
            bail!("An account's provider cannot change.");
        }
        let scopes: Vec<String> = serde_json::from_value(params["scopes"].clone())
            .context("Choose explicit account permissions")?;
        let offered = catalog().as_array().unwrap().iter().any(|p| p["id"] == provider);
        if existing.is_none() && !offered {
            bail!("Choose an account from the connection catalog.");
        }
        let row = catalog_row(provider).ok_or_else(|| anyhow!("Choose an account from the connection catalog."))?;
        let allowed = row["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .chain(row["writeScopes"].as_array().unwrap().iter())
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        if scopes.is_empty() || scopes.iter().any(|s| !allowed.contains(&s.as_str())) {
            bail!("Choose permissions from the provider catalog.");
        }
        let mut scopes = scopes;
        scopes.sort();
        scopes.dedup();
        let writable = params["writable"].as_bool().unwrap_or(false);
        if !writable
            && scopes.iter().any(|s| {
                row["writeScopes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|x| x.as_str() == Some(s))
            })
        {
            bail!("Write permissions require enabling account writes.");
        }
        let endpoints: Endpoints = if let Some(value) = params.get("endpoints") {
            serde_json::from_value(value.clone())?
        } else {
            existing
                .as_ref()
                .map(|a| a.endpoints.clone())
                .unwrap_or(default)
        };
        for url in [&endpoints.authorize, &endpoints.token, &endpoints.api] {
            endpoint(url)?;
        }
        if !endpoints.revoke.is_empty() {
            endpoint(&endpoints.revoke)?;
        }
        let client_id = params["clientId"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();
        if client_id.is_empty() {
            bail!("Register an OAuth app and enter its Client ID.");
        }
        let secret = params["clientSecret"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| {
                existing
                    .as_ref()
                    .map(|a| a.client_secret.clone())
                    .unwrap_or_default()
            });
        // New app credentials or endpoints need a fresh sign-in. New permissions keep the
        // current grant working until the user signs in again to add them.
        let changed = existing.as_ref().is_none_or(|a| {
            a.client_id != client_id
                || a.client_secret != secret
                || serde_json::to_value(&a.endpoints).ok() != serde_json::to_value(&endpoints).ok()
        });
        let widened = existing.as_ref().is_some_and(|a| {
            !a.access_token.is_empty() && scopes.iter().any(|s| !scope_granted(&a.granted_scopes, s))
        });
        let mut account = existing.clone().unwrap_or(Account {
            id: format!("acct-{}", uuid::Uuid::new_v4().simple()),
            name: String::new(),
            provider: provider.into(),
            client_id: String::new(),
            client_secret: String::new(),
            scopes: Vec::new(),
            granted_scopes: Vec::new(),
            writable,
            enabled: true,
            endpoints: endpoints.clone(),
            access_token: String::new(),
            refresh_token: String::new(),
            expires: None,
            status: "disconnected".into(),
            identity: String::new(),
            checked: None,
            error: None,
            generation: 0,
        });
        account.name = params["name"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(provider)
            .trim()
            .into();
        account.client_id = client_id;
        account.client_secret = secret;
        account.scopes = scopes;
        account.writable = writable;
        account.enabled = params["enabled"].as_bool().unwrap_or(true);
        account.endpoints = endpoints;
        account.generation += 1;
        if changed {
            account.access_token.clear();
            account.refresh_token.clear();
            account.expires = None;
            account.status = "disconnected".into();
            account.granted_scopes.clear();
            account.checked = None;
        } else if widened {
            account.status = "consent".into();
            account.error = Some("Sign in again to grant the newly selected permissions.".into());
        } else if account.status == "consent" {
            account.status = "authorized".into();
            account.error = None;
        }
        let mut list = self.list.lock().unwrap();
        let mut next = list.clone();
        if let Some(slot) = next.iter_mut().find(|a| a.id == account.id) {
            *slot = account.clone();
        } else {
            next.push(account.clone());
        }
        durable::save(&self.path, &next)?;
        *list = next;
        Ok(account.view())
    }
    pub fn authorize(&self, id: &str, origin: &str) -> Result<Value> {
        let account = self.get(id)?;
        if !supported(&account.provider) {
            bail!("This account type is no longer supported; remove it.");
        }
        if !account.enabled {
            bail!("Resume this account first.");
        }
        let redirect = format!("{}/oauth/callback", origin.trim_end_matches('/'));
        endpoint(&redirect)?;
        let state = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let verifier = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        let mut url = endpoint(&account.endpoints.authorize)?;
        url.query_pairs_mut().extend_pairs([
            ("client_id", account.client_id.as_str()),
            ("redirect_uri", &redirect),
            ("response_type", "code"),
            ("scope", &account.scopes.join(" ")),
            ("state", &state),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ]);
        if matches!(account.provider.as_str(), "google" | "gmail" | "calendar" | "drive") {
            url.query_pairs_mut()
                .extend_pairs([("access_type", "offline"), ("prompt", "consent")]);
        }
        if account.provider == "google" {
            url.query_pairs_mut().append_pair("include_granted_scopes", "true");
        }
        let mut pending = self.pending.lock().unwrap();
        pending.retain(|_, p| p.expires > now_ms());
        pending.retain(|_, p| p.account != id);
        pending.insert(
            state,
            Pending {
                account: id.into(),
                verifier,
                redirect: redirect.clone(),
                expires: now_ms() + 600_000,
                generation: account.generation,
            },
        );
        Ok(json!({"url":url.to_string(),"redirect":redirect}))
    }
    pub async fn callback(&self, state: &str, code: &str, denied: bool) -> Result<String> {
        let pending = self.pending.lock().unwrap().remove(state).ok_or_else(|| {
            anyhow!("Authorization expired or was already used; start again from Settings.")
        })?;
        if pending.expires < now_ms() {
            bail!("Authorization expired; start again.");
        }
        let _guard = self.gate.lock().await;
        let mut account = self.get(&pending.account)?;
        if account.generation != pending.generation || !account.enabled {
            bail!("Account settings changed; start authorization again.");
        }
        if denied || code.is_empty() {
            account.status = "disconnected".into();
            account.error = Some("Authorization was declined.".into());
            self.commit(account)?;
            bail!("Authorization was declined.");
        }
        let form = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &pending.redirect),
            ("client_id", &account.client_id),
            ("client_secret", &account.client_secret),
            ("code_verifier", &pending.verifier),
        ];
        let result = response_json(
            client()?
                .post(&account.endpoints.token)
                .header("Accept", "application/json")
                .form(&form)
                .send()
                .await
                .map_err(|_| anyhow!("OAuth token service is unavailable"))?,
        )
        .await;
        match result.and_then(|value| apply_token(&mut account, &value)) {
            Ok(()) => {
                account.status = "authorized".into();
                account.error = None;
                let id = account.id.clone();
                self.commit(account)?;
                Ok(id)
            }
            Err(error) => {
                account.status = "error".into();
                account.error = Some(error.to_string());
                self.commit(account)?;
                Err(error)
            }
        }
    }
    async fn authorized(&self, id: &str) -> Result<Account> {
        let _guard = self.gate.lock().await;
        let mut account = self.get(id)?;
        if !supported(&account.provider) {
            bail!("This account type is no longer supported; remove it.");
        }
        if !account.enabled {
            bail!("This account is paused.");
        }
        if account.access_token.is_empty() {
            bail!("Authorize this account in Settings first.");
        }
        if account
            .expires
            .is_some_and(|time| time <= now_ms() + 60_000)
        {
            if account.refresh_token.is_empty() {
                account.status = "expired".into();
                account.error =
                    Some("Account authorization expired; reconnect in Settings.".into());
                self.commit(account)?;
                bail!("Account authorization expired; reconnect in Settings.");
            }
            let form = [
                ("grant_type", "refresh_token"),
                ("refresh_token", &account.refresh_token),
                ("client_id", &account.client_id),
                ("client_secret", &account.client_secret),
            ];
            let response = client()?
                .post(&account.endpoints.token)
                .header("Accept", "application/json")
                .form(&form)
                .send()
                .await
                .map_err(|_| anyhow!("Token refresh service is unavailable"))?;
            if let Err(error) = response_json(response)
                .await
                .and_then(|value| apply_token(&mut account, &value))
            {
                account.status = "expired".into();
                account.error = Some(error.to_string());
                self.commit(account)?;
                return Err(error);
            }
            self.commit(account.clone())?;
        }
        Ok(account)
    }
    pub async fn request(
        &self,
        id: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value> {
        let method = method.to_uppercase();
        if !["GET", "POST", "PUT", "PATCH", "DELETE"].contains(&method.as_str()) {
            bail!("Unsupported account API method.");
        }
        let account = self.authorized(id).await?;
        if method != "GET" && !account.writable {
            bail!(
                "Account writes are disabled; enable them and grant matching permissions in Settings."
            );
        }
        let url = api_url(&account, path)?;
        service_allowed(&account, &url, &method)?;
        let mut request = client()?
            .request(method.parse()?, url)
            .bearer_auth(&account.access_token)
            .header("Accept", "application/json");
        if let Some(body) = body {
            if body.to_string().len() > 256 * 1024 {
                bail!("Account request body is at most 256 KiB.");
            }
            request = request.json(body);
        }
        let current = self.get(id)?;
        if !current.enabled || current.generation != account.generation {
            bail!("Account access changed; retry after reviewing its settings.");
        }
        let result = response_json(
            request
                .send()
                .await
                .map_err(|_| anyhow!("Account service is unavailable"))?,
        )
        .await;
        match &result {
            Ok(_) => {}
            Err(error) => {
                let mut current = self.get(id)?;
                current.error = Some(error.to_string());
                current.status = "error".into();
                self.commit(current)?;
            }
        }
        result.map(|value| {
            redact_json(
                value,
                &[
                    &account.access_token,
                    &account.refresh_token,
                    &account.client_secret,
                ],
            )
        })
    }
    pub async fn test(&self, id: &str) -> Result<Value> {
        let account = self.get(id)?;
        let value = match self
            .request(id, "GET", &account.endpoints.identity, None)
            .await
        {
            Ok(value) => value,
            Err(error) => {
                let mut current = self.get(id)?;
                current.checked = Some(now_ms());
                current.error = Some(error.to_string());
                current.status = "error".into();
                self.commit(current)?;
                return Err(error);
            }
        };
        let mut current = self.get(id)?;
        current.checked = Some(now_ms());
        current.status = "connected".into();
        current.error = None;
        current.identity = value["emailAddress"]
            .as_str()
            .or(value["email"].as_str())
            .or(value["login"].as_str())
            .or(value["user"]["emailAddress"].as_str())
            .or(value["userPrincipalName"].as_str())
            .or(value["displayName"].as_str())
            .unwrap_or("API access verified")
            .to_string();
        self.commit(current.clone())?;
        Ok(current.view())
    }
    pub async fn revoke(&self, id: &str, remove: bool) -> Result<Value> {
        let _guard = self.gate.lock().await;
        let mut account = self.get(id)?;
        let mut remote = "No remote token to revoke".to_string();
        if !account.access_token.is_empty() {
            let result: Result<()> = async {
                if !account.endpoints.revoke.is_empty() {
                    let response = client()?.post(&account.endpoints.revoke).form(&[("token", if account.refresh_token.is_empty() { &account.access_token } else { &account.refresh_token })]).send().await.map_err(|_| anyhow!("Provider revocation unavailable"))?;
                    if !response.status().is_success() { bail!("Provider revocation returned HTTP {}",response.status()); }
                } else if account.provider == "github" {
                    let url = api_url(&account, &format!("/applications/{}/grant", urlencoding::encode(&account.client_id)))?;
                    let response = client()?.delete(url).basic_auth(&account.client_id,Some(&account.client_secret)).json(&json!({"access_token":account.access_token})).send().await.map_err(|_| anyhow!("Provider revocation unavailable"))?;
                    if !response.status().is_success() { bail!("GitHub revocation returned HTTP {}",response.status()); }
                } else { bail!("Remove this app's consent in your Microsoft account to revoke the provider grant."); }
                Ok(())
            }.await;
            remote = result
                .map(|_| "Provider grant revoked".into())
                .unwrap_or_else(|e| e.to_string());
        }
        account.access_token.clear();
        account.refresh_token.clear();
        account.expires = None;
        account.granted_scopes.clear();
        account.generation += 1;
        account.status = "disconnected".into();
        account.error = None;
        let mut list = self.list.lock().unwrap();
        let mut next = list.clone();
        if remove {
            next.retain(|a| a.id != id);
        } else if let Some(slot) = next.iter_mut().find(|a| a.id == id) {
            *slot = account;
        }
        durable::save(&self.path, &next)?;
        *list = next;
        self.pending.lock().unwrap().retain(|_, p| p.account != id);
        Ok(json!({"ok":true,"localRevoked":true,"provider":remote}))
    }
}

fn apply_token(account: &mut Account, value: &Value) -> Result<()> {
    let token = value["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            anyhow!("OAuth provider did not grant an access token; check the app configuration.")
        })?;
    account.access_token = token.into();
    if let Some(token) = value["refresh_token"].as_str().filter(|s| !s.is_empty()) {
        account.refresh_token = token.into();
    }
    account.expires = value["expires_in"]
        .as_i64()
        .or_else(|| value["expires_in"].as_str().and_then(|s| s.parse().ok()))
        .map(|seconds| now_ms() + seconds.clamp(0, 31_536_000) * 1000);
    account.granted_scopes = value["scope"]
        .as_str()
        .map(|s| {
            s.split([' ', ','])
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_else(|| account.scopes.clone());
    Ok(())
}
fn redact_json(value: Value, secrets: &[&str]) -> Value {
    match value {
        Value::String(mut s) => {
            for secret in secrets.iter().filter(|s| s.len() >= 4) {
                s = s.replace(secret, "[account credential]");
            }
            Value::String(s)
        }
        Value::Array(a) => Value::Array(a.into_iter().map(|v| redact_json(v, secrets)).collect()),
        Value::Object(o) => Value::Object(
            o.into_iter()
                .map(|(k, v)| (k, redact_json(v, secrets)))
                .collect(),
        ),
        v => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn account_paths_cannot_escape_the_authorized_service() {
        let home = tempfile::tempdir().unwrap();
        let accounts = Accounts::open(home.path()).unwrap();
        let saved=accounts.save(&json!({"provider":"github","clientId":"app","scopes":["read:user"]})).unwrap();
        let mut account = accounts.get(saved["id"].as_str().unwrap()).unwrap();
        account.provider = "gmail".into();
        account.endpoints = defaults("gmail").unwrap();
        for path in [
            "https://evil.invalid/",
            "//evil.invalid/token",
            "/../../token",
            "/../v1-other",
            "/users\\@evil.invalid",
            "/users#fragment",
        ] {
            assert!(api_url(&account, path).is_err(), "{path}");
        }
        assert_eq!(
            api_url(&account, "/users/me/messages?maxResults=10")
                .unwrap()
                .as_str(),
            "https://gmail.googleapis.com/gmail/v1/users/me/messages?maxResults=10"
        );
        assert!(endpoint("http://192.168.1.1/token").is_err());
        assert!(endpoint("https://user:password@example.com/").is_err());
        assert!(endpoint("http://[::1]:123/token").is_ok());
    }
    #[test]
    fn permissions_and_failed_saves_do_not_silently_change_authorization() {
        let home = tempfile::tempdir().unwrap();
        let accounts = Accounts::open(home.path()).unwrap();
        assert!(
            accounts
                .save(&json!({"provider":"github","clientId":"app","scopes":["repo"]}))
                .is_err()
        );
        let saved=accounts.save(&json!({"provider":"github","clientId":"app","clientSecret":"private-client-key","scopes":["read:user"]})).unwrap();
        assert!(!saved.to_string().contains("private-client-key"));
        std::fs::create_dir(home.path().join("accounts.json.tmp")).unwrap();
        assert!(accounts.save(&json!({"id":saved["id"],"provider":"github","clientId":"app","scopes":["read:user"],"enabled":false})).is_err());
        assert!(accounts.get(saved["id"].as_str().unwrap()).unwrap().enabled);
    }
    #[test]
    fn single_service_google_accounts_are_not_offered_but_keep_working() {
        let home = tempfile::tempdir().unwrap();
        let accounts = Accounts::open(home.path()).unwrap();
        for provider in ["calendar", "gmail", "drive"] {
            assert!(accounts.save(&json!({"provider":provider,"clientId":"app","scopes":["https://www.googleapis.com/auth/calendar.readonly"]})).is_err(), "{provider}");
        }
        let saved=accounts.save(&json!({"provider":"github","clientId":"app","scopes":["read:user"]})).unwrap();
        let path = home.path().join("accounts.json");
        let legacy = std::fs::read_to_string(&path).unwrap().replace("\"provider\": \"github\"", "\"provider\": \"calendar\"").replace("\"scopes\": [\n      \"read:user\"\n    ]", "\"scopes\": [\n      \"https://www.googleapis.com/auth/calendar.readonly\"\n    ]");
        std::fs::write(&path, legacy).unwrap();
        let accounts = Accounts::open(home.path()).unwrap();
        let id = saved["id"].as_str().unwrap();
        assert_eq!(accounts.get(id).unwrap().provider, "calendar");
        assert!(accounts.authorize(id, "http://127.0.0.1:4870").is_ok());
        let paused = accounts.get(id).unwrap();
        assert!(accounts.save(&json!({"id":id,"provider":"calendar","clientId":"app","scopes":paused.scopes,"enabled":false})).is_ok());
    }
    #[test]
    fn google_services_follow_the_permissions_chosen_after_sign_in() {
        let home = tempfile::tempdir().unwrap();
        let accounts = Accounts::open(home.path()).unwrap();
        assert!(accounts.connect("google", "http://127.0.0.1:4870").is_err());
        accounts.save_app(&json!({"provider":"google","clientId":"app.apps.googleusercontent.com","clientSecret":"app-secret"})).unwrap();
        let started = accounts.connect("google", "http://127.0.0.1:4870").unwrap();
        assert!(started["url"].as_str().unwrap().contains("include_granted_scopes=true"));
        let id = started["id"].as_str().unwrap();
        let mut account = accounts.get(id).unwrap();
        assert_eq!(account.client_secret, "app-secret");
        let url = |u: &str| api_url(&account, u).unwrap();
        // Read access to Calendar comes with sign-in; writes and Photos do not.
        assert!(service_allowed(&account, &url("https://www.googleapis.com/calendar/v3/users/me/calendarList"), "GET").is_ok());
        assert!(service_allowed(&account, &url("https://www.googleapis.com/calendar/v3/calendars/primary/events"), "POST").is_err());
        assert!(service_allowed(&account, &url("https://photoslibrary.googleapis.com/v1/mediaItems"), "GET").is_err());
        assert!(service_allowed(&account, &url("/oauth2/v3/userinfo"), "GET").is_ok());
        assert!(service_allowed(&account, &url("/youtube/v3/channels"), "GET").is_err());
        assert!(api_url(&account, "https://evil.invalid/calendar/v3").is_err());
        assert!(api_url(&account, "https://www.googleapis.com.evil.invalid/calendar/v3").is_err());
        // Turning Calendar off applies at once; adding Photos asks to sign in again.
        account.access_token = "token".into();
        account.granted_scopes = account.scopes.iter().map(|s| s.replace("email", "https://www.googleapis.com/auth/userinfo.email").replace("profile", "https://www.googleapis.com/auth/userinfo.profile")).collect();
        accounts.commit(account).unwrap();
        let mut scopes: Vec<String> = accounts.get(id).unwrap().scopes.into_iter().filter(|s| !s.contains("calendar")).collect();
        let narrowed = accounts.save(&json!({"id":id,"provider":"google","clientId":"app.apps.googleusercontent.com","scopes":scopes})).unwrap();
        assert_ne!(narrowed["status"], "consent");
        let account = accounts.get(id).unwrap();
        assert!(!account.access_token.is_empty());
        assert!(service_allowed(&account, &api_url(&account, "https://www.googleapis.com/calendar/v3/users/me/calendarList").unwrap(), "GET").is_err());
        scopes.push("https://www.googleapis.com/auth/photoslibrary.readonly.appcreateddata".into());
        let widened = accounts.save(&json!({"id":id,"provider":"google","clientId":"app.apps.googleusercontent.com","scopes":scopes})).unwrap();
        assert_eq!(widened["status"], "consent");
        assert!(!accounts.get(id).unwrap().access_token.is_empty());
    }
}
