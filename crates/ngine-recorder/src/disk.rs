use std::path::Path;

use sysinfo::{DiskRefreshKind, Disks};

/// The bytes still free on the disk that holds `folder`, or `None` when the
/// operating system does not say. A folder that does not exist yet is
/// answered for its nearest existing parent.
pub fn free_space_bytes(folder: &Path) -> Option<u64> {
    let existing = folder
        .ancestors()
        .find_map(|ancestor| ancestor.canonicalize().ok())
        .map(|path| without_verbatim_prefix(&path))?;
    let disks =
        Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_storage()).ok()?;
    disks
        .iter()
        .filter(|disk| existing.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(sysinfo::Disk::available_space)
}

#[cfg(target_os = "windows")]
fn without_verbatim_prefix(path: &Path) -> std::path::PathBuf {
    let text = path.to_string_lossy();
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        std::path::PathBuf::from(format!(r"\\{share}"))
    } else if let Some(local) = text.strip_prefix(r"\\?\") {
        std::path::PathBuf::from(local)
    } else {
        path.to_path_buf()
    }
}

#[cfg(not(target_os = "windows"))]
fn without_verbatim_prefix(path: &Path) -> std::path::PathBuf {
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_temp_folder_and_a_missing_folder_inside_it_sit_on_a_disk_with_a_size() {
        let temp = std::env::temp_dir();
        assert!(free_space_bytes(&temp).is_some());
        assert!(free_space_bytes(&temp.join("ngine-no-such-folder").join("deeper")).is_some());
    }
}
