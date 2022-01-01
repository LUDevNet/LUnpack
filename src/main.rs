use argh::FromArgs;
use assembly_pack::{
    crc::calculate_crc, md5::MD5Sum, pk::reader::PackFile, pki::core::PackIndexFile,
    txt::load_manifest,
};
use globset::{Glob, GlobSetBuilder};
use log::LevelFilter;
use std::{
    collections::BTreeMap,
    error::Error,
    io::{self, Read},
    path::{Path, PathBuf},
};
use thiserror::Error;
use tokio_stream::wrappers::LinesStream;

use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
};

#[derive(FromArgs)]
/// Unpack a LEGO® Universe client
struct Options {
    /// the path of a client
    #[argh(positional)]
    input: Option<PathBuf>,
    /// the path to put files into (defaults to input)
    #[argh(option, short = 'o')]
    output: Option<PathBuf>,
    /// whether to skip actually writing the files
    #[argh(switch, short = 'd', long = "dry-run")]
    dry_run: bool,
    /// globset file
    #[argh(option, short = 'g')]
    glob: Option<PathBuf>,
}

struct Task<'a> {
    i: usize,
    total: usize,
    dry_run: bool,
    output: &'a Path,
    pk_key: String,
    pk_file: &'a Path,
    files: Vec<(u32, String, u32, MD5Sum)>,
}

impl<'a> Task<'a> {
    async fn un_pack_file(self) -> io::Result<()> {
        let short_key = self
            .pk_key
            .strip_prefix("client\\res\\pack\\")
            .unwrap_or(&self.pk_key);
        let file = match std::fs::File::open(self.pk_file) {
            Ok(file) => file,
            Err(_e) => {
                log::warn!("Failed to open {:?}", short_key);
                return Ok(());
            }
        };
        let mut reader = std::io::BufReader::new(file);
        let mut pk = PackFile::open(&mut reader);

        if let Err(e) = pk.check_magic() {
            log::error!("Failed to check PK magic for {:?}:\n {}", short_key, e);
        }

        let trailer = pk.get_header()?;
        let entries = pk.get_entry_list(trailer.file_list_base_addr)?;

        let mut buffer = vec![0u8; 1024 * 16];

        for (crc, file_path, _size, _hash) in self.files {
            match entries
                .as_slice()
                .binary_search_by_key(&crc, |node| node.crc)
            {
                Ok(index) => {
                    let entry = entries.get(index).unwrap();
                    if entry.is_compressed & 0x01 > 0 {
                        log::debug!("Compressed: {}", file_path);
                    }
                    let mut stream = pk.get_file_data(*entry)?;
                    let out_file = self.output.join(&file_path);
                    if self.dry_run {
                        println!("{}", out_file.display());
                    } else {
                        if let Some(parent) = out_file.parent() {
                            tokio::fs::create_dir_all(parent).await?;
                        }
                        match tokio::fs::File::create(&out_file).await {
                            Ok(out) => {
                                let mut writer = tokio::io::BufWriter::new(out);
                                loop {
                                    let len = stream.read(&mut buffer)?;
                                    if len == 0 {
                                        break;
                                    }
                                    writer.write_all(&buffer[..len]).await?;
                                }
                                writer.flush().await?;
                            }
                            Err(e) => {
                                log::error!("Failed to write {:?}: {}", out_file, e);
                            }
                        }
                    }
                }
                Err(_pos) => {
                    log::warn!("Failed to find {:?} in {:?}", file_path, short_key);
                }
            }
        }

        log::info!("{}/{} {:?}", self.i, self.total, short_key);

        Ok(())
    }
}

#[derive(Error)]
pub enum UnpackError {
    #[error("generic I/O error")]
    IO(#[from] io::Error),
    #[error("could not find {0}")]
    FileNotFound(String, #[source] io::Error),
    #[error("unknown error")]
    Unknown,
}

impl std::fmt::Debug for UnpackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{}", self)?;

        let mut src_opt: Option<&dyn std::error::Error> = self.source().as_ref().copied();
        if src_opt.is_some() {
            writeln!(f)?;
            writeln!(f, "Caused by:")?;
        }
        while let Some(src) = src_opt {
            writeln!(f, "\t{}", src)?;
            src_opt = src.source();
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), UnpackError> {
    env_logger::builder()
        .format_timestamp(None)
        .filter_level(LevelFilter::Info)
        .init();

    let opts: Options = argh::from_env();
    let input = match opts.input {
        Some(set) => set,
        None => std::env::current_dir()?,
    };
    let output = opts.output.unwrap_or_else(|| input.clone());

    let download_dir = input.join("versions");
    let trunk = download_dir.join("trunk.txt");
    let index = download_dir.join("primary.pki");

    let mut builder = GlobSetBuilder::new();
    if let Some(globfile) = opts.glob {
        let globs = std::fs::read_to_string(&globfile)?;
        for (num, line) in globs.lines().enumerate() {
            if !line.is_empty() && !line.starts_with('#') {
                match Glob::new(line) {
                    Ok(gl) => {
                        builder.add(gl);
                    }
                    Err(e) => {
                        log::error!("Invalid glob {:?} on line {}: {}", line, num, e);
                    }
                }
            }
        }
    } else {
        builder.add(Glob::new("**").unwrap());
    }
    let globset = builder.build().unwrap();

    let file = File::open(&trunk)
        .await
        .map_err(|e| UnpackError::FileNotFound(trunk.display().to_string(), e))?;
    let reader = BufReader::new(file);
    let mut trunk_lines = LinesStream::new(reader.lines());

    let manifest = load_manifest(&mut trunk_lines).await.unwrap();

    let pack_index = PackIndexFile::try_from(index.as_path()).unwrap();
    let mut tasks = BTreeMap::new();

    for (file, data) in manifest.files {
        if !globset.is_match(&file) {
            continue;
        }
        let crc = calculate_crc(file.as_bytes());
        if let Some(file_ref) = pack_index.files.get(&crc) {
            // This file is in a PK archive
            let archive = &pack_index.archives[file_ref.pack_file as usize];
            let pk_archive = tasks.entry(archive.path.clone()).or_insert_with(Vec::new);
            pk_archive.push((crc, file, data.filesize, data.hash));
        }
    }

    let total = tasks.len();
    for (i, (key, files)) in tasks.into_iter().enumerate() {
        let pack_name = key.replace('\\', "/");
        let pk_file = input.join(pack_name);
        let task = Task {
            i: i + 1,
            total,
            dry_run: opts.dry_run,
            output: &output,
            pk_key: key.clone(),
            pk_file: &pk_file,
            files,
        };
        if let Err(e) = task.un_pack_file().await {
            log::error!("failed to unpack {}:\n  {}", key, e)
        }
    }

    Ok(())
}
