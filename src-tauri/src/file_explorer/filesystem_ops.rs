use crate::util::strings::{
    bytes_to_gb, os_to_string, ostr_to_string, path_to_string, pathbuf_to_string,
};
use crate::{CachedPath, StateSafe};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::fs::{read_dir, File, FileType};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use sysinfo::{DiskExt, System, SystemExt};
use tauri::State;
use walkdir::WalkDir;

const CACHE_FILE_PATH: &str = "./disk_cache.json";

#[derive(Serialize)]
pub struct Disk {
    name: String,
    available_gb: u16,
    used_gb: u16,
    total_gb: u16,
    fs: DiskFileSystem,
}

#[derive(Serialize)]
pub struct DiskFileSystem {
    root: PathBuf,
    documents: PathBuf,
    downloads: PathBuf,
    pictures: PathBuf,
    videos: PathBuf,
    home: PathBuf,
    audio: PathBuf,
    desktop: PathBuf,
}

impl DiskFileSystem {
    fn try_new() -> Result<Self, ()> {
        macro_rules! handle_err {
            ($func:expr) => {
                match $func {
                    Some(dir) => dir,
                    None => return Err(()),
                }
            };
        }

        let documents = handle_err!(dirs::document_dir());
        let downloads = handle_err!(dirs::download_dir());
        let pictures = handle_err!(dirs::picture_dir());
        let videos = handle_err!(dirs::video_dir());
        let home = handle_err!(dirs::home_dir());
        let audio = handle_err!(dirs::audio_dir());
        let desktop = handle_err!(dirs::desktop_dir());
        let root: PathBuf = {
            #[cfg(target_family = "unix")]
            {
                "/".into()
            }
            #[cfg(target_family = "windows")]
            {
                "C:".into()
            }
        };

        Ok(Self {
            root,
            documents,
            downloads,
            pictures,
            videos,
            home,
            audio,
            desktop,
        })
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub enum DirectoryChild {
    File(String, String), // Name of file, path to file
    Directory(String, String),
}

/// Gets the cache from the state (in memory), encodes and saves it to the cache file path.
/// This needs optimising.
pub fn save_cache_to_disk(state_mux: &StateSafe) {
    let state = &mut state_mux.lock().unwrap();
    let serialized_cache = serde_json::to_string(&state.disk_cache).unwrap();

    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(CACHE_FILE_PATH)
        .unwrap();
    file.write_all(serialized_cache.as_bytes()).unwrap();
}

/// This traverses the provided disk and adds the file structure to the cache in memory.
pub fn cache_disk(state_mux: &StateSafe, path: &Path, fs_root: PathBuf) {
    let state = &mut state_mux.lock().unwrap();

    let disk_cache = state
        .disk_cache
        .entry(fs_root.to_str().unwrap().into())
        .or_insert_with(HashMap::new);

    let disk_cache = Arc::new(Mutex::new(disk_cache));

    WalkDir::new(path)
        .into_iter()
        .par_bridge()
        .filter_map(|entry| entry.ok())
        .for_each(|entry| {
            let file_name = ostr_to_string(entry.file_name());
            let file_path = path_to_string(entry.path());

            let walkdir_filetype = entry.file_type();
            let mut file_type = String::from("file");
            if FileType::is_dir(&walkdir_filetype) {
                file_type = String::from("directory");
            }

            let cache_guard = &mut disk_cache.lock().unwrap();
            cache_guard
                .entry(file_name)
                .or_insert_with(Vec::new)
                .push(CachedPath {
                    file_path,
                    file_type,
                });
        });
}

/// Reads and decodes the cache file and stores it in memory for quick access.
pub fn load_cache(state_mux: &StateSafe) {
    let state = &mut state_mux.lock().unwrap();
    let file_contents = fs::read_to_string(CACHE_FILE_PATH).unwrap();
    state.disk_cache = serde_json::from_str(&file_contents).unwrap();
}

/// Gets list of disk partitions and returns them.
/// If there is a cache stored on disk it is loaded.
/// If there is no cache stored on disk, one is created as well as stored in memory.
#[tauri::command]
pub fn get_disks(state_mux: State<'_, StateSafe>) -> Vec<Disk> {
    let mut disks = Vec::new();

    let mut sys = System::new_all();
    sys.refresh_all();

    let cache_exists = fs::metadata(CACHE_FILE_PATH).is_ok();
    if cache_exists {
        load_cache(&state_mux);
    } else {
        File::create(CACHE_FILE_PATH).unwrap();
    }

    for disk in sys.disks() {
        let used_bytes = disk.total_space() - disk.available_space();
        let available_gb = bytes_to_gb(disk.available_space());
        let used_gb = bytes_to_gb(used_bytes);
        let total_gb = bytes_to_gb(disk.total_space());

        let mut name = ostr_to_string(disk.name());
        if name.is_empty() {
            name = String::from("Local Disk");
        }

        let mnt_point = disk.mount_point();

        let fs = DiskFileSystem::try_new().unwrap();

        if !cache_exists {
            cache_disk(&state_mux, mnt_point, fs.root.clone());
            save_cache_to_disk(&state_mux);
        }

        disks.push(Disk {
            name,
            available_gb,
            used_gb,
            total_gb,
            fs,
        });
    }

    disks
}

/// Searches and returns the files in a given directory. This is not recursive.
#[tauri::command]
pub fn open_directory(path: String) -> Vec<DirectoryChild> {
    let mut dir_children = Vec::new();

    let Ok(directory) = read_dir(path) else {
        return dir_children;
    };

    for entry in directory {
        let entry = entry.unwrap();
        let file_name = os_to_string(entry.file_name());

        if entry.file_type().unwrap().is_file() {
            dir_children.push(DirectoryChild::File(
                file_name,
                pathbuf_to_string(entry.path()),
            ));
            continue;
        }

        dir_children.push(DirectoryChild::Directory(
            file_name,
            pathbuf_to_string(entry.path()),
        ));
    }

    dir_children
}