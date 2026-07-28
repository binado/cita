use super::Target;
use anyhow::Result;

pub(crate) fn remove(target: &Target<'_>, selectors: &[String]) -> Result<()> {
    for item in target.library().remove_batch(target.name(), selectors)? {
        println!("Removed {}", item.key);
    }
    Ok(())
}
