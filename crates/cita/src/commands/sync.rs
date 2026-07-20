use super::{find_manifest, inspire_client};
use anyhow::Result;
use cita_core::MetadataProvider;
use cita_manifest::Manifest;
use std::path::Path;

pub(crate) async fn sync(cwd: &Path) -> Result<()> {
    let mut manifest = Manifest::load_verified(find_manifest(cwd)?)?;
    let ids = manifest.inspire_record_ids();
    let managed = ids.len();
    let unmanaged = manifest.references().len() - managed;
    let provider_ids = ids.iter().map(u64::to_string).collect::<Vec<_>>();
    let refreshed = inspire_client()?.refresh(&provider_ids).await?;
    let changed = manifest.replace_inspire(refreshed)?;
    if changed {
        println!("Synced {managed} managed references; left {unmanaged} imported unchanged");
    } else {
        println!("Already in sync: {managed} managed, {unmanaged} imported");
    }
    Ok(())
}
