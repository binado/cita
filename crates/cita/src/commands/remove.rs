use super::Target;
use anyhow::Result;

pub(crate) fn remove(target: &Target, selectors: &[String]) -> Result<()> {
    let _lock = target.lock()?;
    let mut manifest = target.load()?;
    for item in manifest.remove_batch(selectors)? {
        println!("Removed {}", item.key);
    }
    Ok(())
}
