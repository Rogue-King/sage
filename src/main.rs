use age::cli_common;
use age::cli_common::StdinGuard;
use anyhow::{Context, Result, anyhow};
use clap::Parser;
use log::{debug, error, info, warn};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

/// A tool to compress, encrypt, and add error correction to a file or directory.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Encrypt (protect) the input. Mutually exclusive with --decrypt.
    #[arg(
        short = 'e',
        long = "encrypt",
        conflicts_with = "decrypt",
        required_unless_present = "decrypt"
    )]
    encrypt: bool,

    /// Decrypt (recover) the input. Mutually exclusive with --encrypt.
    #[arg(
        short = 'd',
        long = "decrypt",
        conflicts_with = "encrypt",
        required_unless_present = "encrypt"
    )]
    decrypt: bool,

    /// Path to the input file or directory to protect
    #[arg(value_name = "INPUT", required = true)]
    input: PathBuf,

    /// Path for the output protected file
    #[arg(short = 'o', long = "output", value_name = "OUTPUT")]
    output: PathBuf,

    /// Encrypt to the specified RECIPIENT. Can be repeated.
    #[arg(short = 'r', long, value_name = "RECIPIENT", required = false, num_args = 0..)]
    recipient: Vec<String>,

    /// Encrypt to recipients listed at PATH. Can be repeated.
    #[arg(short = 'R', long, value_name = "RECIPIENTS_FILE", required = false, num_args = 0..)]
    recipients_file: Vec<String>,

    /// Path to the identity file
    #[arg(short = 'i', long, value_name = "IDENTITY_FILE")]
    identity_file: Vec<String>,
}

fn main() -> Result<()> {
    env_logger::init();

    let cli = Cli::parse();

    if cli.encrypt {
        info!("Protecting: {}", cli.input.display());
        if let Err(e) = protect(
            &cli.input,
            &cli.output,
            cli.recipient,
            cli.recipients_file,
            cli.identity_file,
        ) {
            error!("Failed to protect file: {e}");
            return Err(e);
        }
        info!("Successfully protected file to: {}", cli.output.display());
    } else if cli.decrypt {
        info!("Recovering file: {}", cli.input.display());
        if let Err(e) = recover(&cli.input, &cli.output, cli.identity_file) {
            error!("Failed to recover file: {e}");
            return Err(e);
        }
        info!("Successfully recovered to: {}", cli.output.display());
    } else {
        warn!("Neither --encrypt nor --decrypt specified.");
        return Err(anyhow!(
            "You must specify either --encrypt (-e) or --decrypt (-d)."
        ));
    }

    Ok(())
}

fn protect(
    input_path: &Path,
    output_path: &Path,
    recipient_strings: Vec<String>,
    recipients_file_strings: Vec<String>,
    identity_strings: Vec<String>,
) -> Result<()> {
    let max_work_factor: Option<u8> = Some(15);
    let mut stdin_guard = StdinGuard::new(true);

    let recipients: Vec<Box<dyn age::Recipient>> = cli_common::read_recipients(
        recipient_strings,
        recipients_file_strings,
        identity_strings,
        max_work_factor,
        &mut stdin_guard,
    )
    .into_iter()
    .flatten()
    .map(|r| {
        let raw: *mut dyn age::Recipient = Box::into_raw(r) as *mut dyn age::Recipient;
        unsafe { Box::from_raw(raw) }
    })
    .collect();

    if recipients.is_empty() {
        warn!("No valid recipients provided.");
        return Err(anyhow!("No valid recipients provided."));
    }

    debug!("Creating output file: {}", output_path.display());
    let output_file = File::create(output_path)
        .with_context(|| format!("Failed to create output file: {}", output_path.display()))?;

    debug!("Initializing age encryption.");
    let encryptor = age::Encryptor::with_recipients(recipients.iter().map(|r| r.as_ref()))?;
    let mut age_writer = encryptor.wrap_output(output_file)?;

    debug!("Initializing zstd compression.");
    let mut zstd_encoder =
        zstd::Encoder::new(&mut age_writer, 0).context("Failed to create zstd encoder")?;

    debug!("Archiving input {} into tar stream.", input_path.display());
    {
        let mut tar_builder = tar::Builder::new(&mut zstd_encoder);
        if input_path.is_dir() {
            tar_builder
                .append_dir_all(".", input_path)
                .with_context(|| format!("Failed to archive directory {}", input_path.display()))?;
            debug!("Directory archived successfully: {}", input_path.display());
        } else {
            let mut file = File::open(input_path).context("Failed to open input file")?;
            let filename = input_path
                .file_name()
                .ok_or_else(|| anyhow!("Invalid input file name"))?
                .to_string_lossy();
            tar_builder.append_file(Path::new(filename.as_ref()), &mut file)?;
            debug!("File archived successfully: {}", input_path.display());
        }
        tar_builder.finish()?;
    }

    debug!("Finishing compression and encryption streams.");
    zstd_encoder.finish()?;
    age_writer.finish()?;

    debug!(
        "Protection complete. Output written to: {}",
        output_path.display()
    );

    Ok(())
}

/// The core recovery pipeline: correct errors -> decrypt -> decompress -> extract.
fn recover(input_path: &Path, output_path: &Path, identity_strings: Vec<String>) -> Result<()> {
    let max_work_factor: Option<u8> = Some(15);
    let mut stdin_guard = StdinGuard::new(true);

    let identities: Vec<Box<dyn age::Identity>> =
        cli_common::read_identities(identity_strings, max_work_factor, &mut stdin_guard)?;

    if identities.is_empty() {
        warn!("No valid identities provided.");
        return Err(anyhow!("No valid identities provided."));
    }

    debug!("Opening encrypted input file: {}", input_path.display());
    let input_file = File::open(input_path)
        .with_context(|| format!("Failed to open input file: {}", input_path.display()))?;

    debug!("Initializing age decryption.");
    let decryptor =
        age::Decryptor::new(input_file)?.decrypt(identities.iter().map(|i| i.as_ref()))?;

    debug!("Initializing zstd decompression.");
    let mut zstd_decoder =
        zstd::Decoder::new(decryptor).context("Failed to create zstd decoder")?;

    debug!(
        "Extracting tar archive to output path: {}",
        output_path.display()
    );
    let mut archive = tar::Archive::new(&mut zstd_decoder);

    if let Some(parent) = output_path.parent()
        && !parent.exists()
    {
        debug!(
            "Output directory does not exist. Creating: {}",
            parent.display()
        );
        fs::create_dir_all(parent)?;
    }
    archive.unpack(output_path)?;
    debug!(
        "Recovery complete. Files extracted to: {}",
        output_path.display()
    );

    Ok(())
}
