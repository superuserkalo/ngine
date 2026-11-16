use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const BOX_UUID: [u8; 16] = [
    0x6e, 0x67, 0x69, 0x6e, 0x65, 0x2d, 0x6d, 0x61, 0x72, 0x6b, 0x65, 0x72, 0x73, 0x2d, 0x76, 0x31,
];
const BOX_TYPE: [u8; 4] = *b"uuid";
const VERSION: u8 = 1;
const BOX_HEADER_BYTES: u64 = 8;
const LARGE_BOX_HEADER_BYTES: u64 = 16;
const PAYLOAD_HEADER_BYTES: usize = 16 + 1 + 4;
const MOST_MARKERS: usize = 100_000;

/// The text file that holds the markers of a recording that is not finished.
pub fn journal_path(recording: &Path) -> PathBuf {
    let mut name = OsString::from(recording.as_os_str());
    name.push(".markers");
    PathBuf::from(name)
}

pub(crate) struct Journal {
    path: PathBuf,
    file: Option<File>,
    markers: Vec<u64>,
}

impl Journal {
    pub(crate) fn new(recording: &Path) -> Self {
        Self {
            path: journal_path(recording),
            file: None,
            markers: Vec::new(),
        }
    }

    pub(crate) fn add(&mut self, micros: u64) -> io::Result<()> {
        if self.markers.len() >= MOST_MARKERS {
            return Ok(());
        }
        self.markers.push(micros);
        if self.file.is_none() {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            self.file = Some(file);
        }
        match self.file.as_mut() {
            Some(file) => {
                writeln!(file, "{micros}")?;
                file.flush()
            }
            None => Ok(()),
        }
    }

    pub(crate) fn seal_into(mut self, recording: &Path) -> io::Result<()> {
        if self.markers.is_empty() {
            return Ok(());
        }
        drop(self.file.take());
        append_box(recording, &self.markers)?;
        fs::remove_file(&self.path)
    }
}

pub(crate) fn append_box(recording: &Path, markers: &[u64]) -> io::Result<()> {
    let count = u32::try_from(markers.len()).map_err(io::Error::other)?;
    let size = BOX_HEADER_BYTES + PAYLOAD_HEADER_BYTES as u64 + u64::from(count) * 8;
    let size = u32::try_from(size).map_err(io::Error::other)?;
    let mut bytes = Vec::with_capacity(size as usize);
    bytes.extend_from_slice(&size.to_be_bytes());
    bytes.extend_from_slice(&BOX_TYPE);
    bytes.extend_from_slice(&BOX_UUID);
    bytes.push(VERSION);
    bytes.extend_from_slice(&count.to_be_bytes());
    for marker in markers {
        bytes.extend_from_slice(&marker.to_be_bytes());
    }
    let mut file = OpenOptions::new().append(true).open(recording)?;
    file.write_all(&bytes)?;
    file.sync_data()
}

fn read_box(file: &mut File) -> io::Result<Option<Vec<Duration>>> {
    let length = file.metadata()?.len();
    let mut at = 0_u64;
    while at + BOX_HEADER_BYTES <= length {
        file.seek(SeekFrom::Start(at))?;
        let mut header = [0_u8; 8];
        file.read_exact(&mut header)?;
        let [s0, s1, s2, s3, kind @ ..] = header;
        let (size, header_bytes) = match u32::from_be_bytes([s0, s1, s2, s3]) {
            0 => (length - at, BOX_HEADER_BYTES),
            1 => {
                let mut large = [0_u8; 8];
                file.read_exact(&mut large)?;
                (u64::from_be_bytes(large), LARGE_BOX_HEADER_BYTES)
            }
            size => (u64::from(size), BOX_HEADER_BYTES),
        };
        if size < header_bytes || at + size > length {
            return Ok(None);
        }
        if kind == BOX_TYPE && size - header_bytes >= PAYLOAD_HEADER_BYTES as u64 {
            let mut payload_header = [0_u8; PAYLOAD_HEADER_BYTES];
            file.read_exact(&mut payload_header)?;
            let (uuid, rest) = payload_header.split_at(16);
            if uuid == BOX_UUID && rest[0] == VERSION {
                let count = u32::from_be_bytes([rest[1], rest[2], rest[3], rest[4]]) as usize;
                let room = (size - header_bytes - PAYLOAD_HEADER_BYTES as u64) / 8;
                if count as u64 > room || count > MOST_MARKERS {
                    return Ok(None);
                }
                let mut bytes = vec![0_u8; count * 8];
                file.read_exact(&mut bytes)?;
                let markers = bytes
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|micros| Duration::from_micros(u64::from_be_bytes(*micros)))
                    .collect();
                return Ok(Some(markers));
            }
        }
        at += size;
    }
    Ok(None)
}

fn read_journal(recording: &Path) -> io::Result<Vec<Duration>> {
    let file = match File::open(journal_path(recording)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut markers = Vec::new();
    for line in BufReader::new(file).lines().take(MOST_MARKERS) {
        if let Ok(micros) = line?.trim().parse() {
            markers.push(Duration::from_micros(micros));
        }
    }
    Ok(markers)
}

/// The markers of a recording, as times from its start, in the order they
/// were dropped. Empty when it has none.
pub fn read_markers(recording: &Path) -> io::Result<Vec<Duration>> {
    let mut file = File::open(recording)?;
    match read_box(&mut file)? {
        Some(markers) => Ok(markers),
        None => read_journal(recording),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ngine-markers-{}-{name}", std::process::id()))
    }

    fn other_box(kind: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut bytes = u32::try_from(payload.len() + 8)
            .unwrap()
            .to_be_bytes()
            .to_vec();
        bytes.extend_from_slice(&kind);
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn markers_are_read_from_the_text_file_until_the_recording_is_sealed() {
        let recording = temp_file("sealed");
        let mut content = other_box(*b"ftyp", b"isom");
        content.extend(other_box(*b"uuid", &[7; 30]));
        content.extend(other_box(*b"mdat", &[0; 100]));
        fs::write(&recording, &content).unwrap();

        let mut journal = Journal::new(&recording);
        journal.add(1_500_000).unwrap();
        journal.add(62_000_000).unwrap();
        let expected = [Duration::from_millis(1_500), Duration::from_secs(62)];
        assert_eq!(read_markers(&recording).unwrap(), expected);
        assert!(journal_path(&recording).exists());

        journal.seal_into(&recording).unwrap();
        assert!(!journal_path(&recording).exists());
        assert_eq!(read_markers(&recording).unwrap(), expected);
        assert_eq!(fs::read(&recording).unwrap()[..content.len()], content);
        fs::remove_file(&recording).unwrap();
    }

    #[test]
    fn a_recording_without_markers_gets_no_box_and_a_damaged_file_reads_as_none() {
        let recording = temp_file("none");
        fs::write(&recording, other_box(*b"ftyp", b"isom")).unwrap();
        Journal::new(&recording).seal_into(&recording).unwrap();
        assert_eq!(fs::read(&recording).unwrap().len(), 12);
        assert_eq!(read_markers(&recording).unwrap(), []);

        fs::write(&recording, [0, 0, 0, 200, b'm', b'o', b'o', b'v', 1, 2]).unwrap();
        assert_eq!(read_markers(&recording).unwrap(), []);
        fs::remove_file(&recording).unwrap();
    }
}
