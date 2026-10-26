//! Owner-only atomic snapshots. Callers commit their in-memory copy only after save.
use anyhow::Result;
use serde::Serialize;
use std::{io::Write, path::Path};

pub fn save(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(result) = crate::sqlite_storage::save(path, &serde_json::to_vec(value)?) { return result; }
    if let Some(parent)=path.parent(){std::fs::create_dir_all(parent)?;}
    let temporary = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub fn save_bytes(path:&Path,bytes:&[u8])->Result<()> {
    let temp=path.with_file_name(format!(".dotbot-write-{}",uuid::Uuid::new_v4().simple()));
    let result=(||->Result<()>{let mut file=std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;file.write_all(bytes)?;if let Ok(meta)=std::fs::metadata(path){file.set_permissions(meta.permissions())?;}file.sync_all()?;std::fs::rename(&temp,path)?;Ok(())})();
    let _=std::fs::remove_file(temp);result
}
