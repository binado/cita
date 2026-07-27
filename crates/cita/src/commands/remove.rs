use super::Target;
use anyhow::Result;

pub(crate) fn remove(target: &Target<'_>, selectors: &[String]) -> Result<()> {
    let lock = target.lock()?;
    let mut manifest = lock.manifest()?;
    for item in manifest.remove_batch(selectors)? {
        println!("Removed {}", item.key);
    }
    Ok(())
}
