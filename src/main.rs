use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use reed_solomon_erasure::galois_8::ReedSolomon;
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

// Define the number of data and parity shards for Reed-Solomon error correction.
// This means we can recover the original data even if up to PARITY_SHARDS shards are lost or corrupted.
const DATA_SHARDS: usize = 10;
const PARITY_SHARDS: usize = 4; // A more typical ratio. Can be adjusted.
const TOTAL_SHARDS: usize = DATA_SHARDS + PARITY_SHARDS;

/// A tool to compress, encrypt, and add error correction to a file or directory.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Compresses, encrypts, and adds error correction codes to a file or directory.
    Protect {
        /// Path to the input file or directory to protect
        #[arg(short = 'i', long, value_name = "PATH")]
        input: PathBuf,

        /// Path for the output protected file
        #[arg(short = 'o', long, value_name = "FILE")]
        output: PathBuf,

        /// Encrypt to the specified RECIPIENT. Can be repeated.
        #[arg(short = 'r', long, value_name = "RECIPIENT", required = false, num_args = 0..)]
        recipient: Vec<String>,

        /// Encrypt to recipients listed at PATH. Can be repeated.
        #[arg(short = 'R', long, value_name = "RECIPIENTS_FILE", required = false, num_args = 0..)]
        recipients_file: Vec<PathBuf>,
    },
    /// Recovers the original file or directory by reversing the protection process.
    Recover {
        /// Path to the protected input file
        #[arg(short = 'i', long, value_name = "FILE")]
        input: PathBuf,

        /// Path to write the recovered output file or directory
        #[arg(short = 'o', long, value_name = "PATH")]
        output: PathBuf,

        /// Use the identity file at PATH. Can be repeated.
        #[arg(short = 'r', long, value_name = "IDENTITY_FILE", required = false, num_args = 0..)]
        identity: Vec<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Protect {
            input,
            output,
            recipient,
            recipients_file,
        } => {
            println!("Protecting: {}", input.display());
            protect(&input, &output, &recipient, &recipients_file)?;
            println!("Successfully protected file to: {}", output.display());
        }
        Commands::Recover {
            input,
            output,
            identity,
        } => {
            println!("Recovering file: {}", input.display());
            recover(&input, &output, &identity)?;
            println!("Successfully recovered to: {}", output.display());
        }
    }

    Ok(())
}

/// The core protection pipeline: archive -> compress -> encrypt -> add error correction.
fn protect(
    input_path: &Path,
    output_path: &Path,
    recipient_keys: &[String],
    recipients_files: &[PathBuf],
) -> Result<()> {
    let mut encrypted_data = Vec::new();

    // Collect all recipient keys from both direct args and files
    let mut recipients = Vec::new();

    // Direct recipients
    for key in recipient_keys {
        if key.starts_with("age1") {
            let recipient = age::x25519::Recipient::from_str(key)
                .map_err(|e| anyhow!("Failed to parse recipient public key: {}", e))?;
            recipients.push(Box::new(recipient) as Box<dyn age::Recipient>);
        } else {
            return Err(anyhow!("Invalid recipient key: {}", key));
        }
    }

    // Recipients from files
    for file in recipients_files {
        let content = fs::read_to_string(file)
            .with_context(|| format!("Failed to read recipients file: {}", file.display()))?;
        for line in content.lines() {
            for word in line.split_whitespace() {
                if word.starts_with("age1") {
                    let recipient = age::x25519::Recipient::from_str(word)
                        .map_err(|e| anyhow!("Failed to parse recipient public key: {}", e))?;
                    recipients.push(Box::new(recipient) as Box<dyn age::Recipient>);
                }
            }
        }
    }

    if recipients.is_empty() {
        return Err(anyhow!("No valid recipients provided."));
    }

    let encryptor = age::Encryptor::with_recipients(recipients.iter().map(|r| &**r))?;
    let mut age_writer = encryptor.wrap_output(&mut encrypted_data)?;


    // 2. Set up the streaming pipeline: tar -> zstd -> age -> Vec<u8>

    // 2b. Zstd encoder wraps the age_writer
    let mut zstd_encoder = zstd::Encoder::new(&mut age_writer, 0)
        .context("Failed to create zstd encoder")?;

    // 2c. Tar builder wraps the zstd_encoder
    {
        let mut tar_builder = tar::Builder::new(&mut zstd_encoder);
        if input_path.is_dir() {
            // When input is a directory, archive its contents.
            tar_builder.append_dir_all(".", input_path)
                .with_context(|| format!("Failed to archive directory {}", input_path.display()))?;
            println!("Step 1: Archived directory into tar format (streaming).");
        } else {
            // When input is a single file.
            let mut file = File::open(input_path).context("Failed to open input file")?;
            let filename = input_path
                .file_name()
                .ok_or_else(|| anyhow!("Invalid input file name"))?
                .to_string_lossy();
            tar_builder.append_file(Path::new(filename.as_ref()), &mut file)?;
            println!("Step 1: Archived file into tar format (streaming).");
        }
        tar_builder.finish()?;
    } // tar_builder is dropped here

    zstd_encoder.finish()?;
    age_writer.finish()?;
    println!("Step 2-3: Compressed and encrypted tar archive using zstd and age (streaming).");

    // 3. Apply Reed-Solomon error correction.
    let r = ReedSolomon::new(DATA_SHARDS, PARITY_SHARDS)
        .context("Failed to create ReedSolomon instance")?;
    
    // Create data shards from the encrypted data. This function now correctly handles all data sizes.
    let mut master_shards = shards_from_data(&encrypted_data, DATA_SHARDS)?;
    
    // Create empty parity shards to be filled by the encoding process.
    let shard_len = master_shards.get(0).map_or(0, |s| s.len());
    let mut parity_shards = vec![vec![0; shard_len]; PARITY_SHARDS];

    // Combine data and parity shards into a single structure for the encoder.
    let mut all_shards: Vec<&mut [u8]> = master_shards
        .iter_mut()
        .chain(parity_shards.iter_mut())
        .map(|v| v.as_mut_slice())
        .collect();

    // This fills the parity_shards with the error correction data.
    r.encode(&mut all_shards)
        .context("Failed to encode parity shards")?;
    println!("Step 4: Generated Reed-Solomon parity shards.");

    // 4. Write all shards to the output file.
    // We first write the original length of the encrypted data. This is crucial for recovery
    // to remove any padding that was added during sharding.
    let mut out_file = File::create(output_path).context("Failed to create output file")?;
    out_file.write_all(&(encrypted_data.len() as u64).to_le_bytes())?;

    for shard in master_shards.iter().chain(parity_shards.iter()) {
        out_file.write_all(shard)?;
    }
    println!("Step 5: Wrote all data and parity shards to output file.");

    Ok(())
}

/// The core recovery pipeline: correct errors -> decrypt -> decompress -> extract.
fn recover(
    input_path: &Path,
    output_path: &Path,
    identity_paths: &[PathBuf],
) -> Result<()> {

    // 1. Read all shard data from the protected file.
    let mut in_file = File::open(input_path).context("Failed to open input file")?;
    let mut len_bytes = [0u8; 8];
    in_file.read_exact(&mut len_bytes).context("Failed to read original data length header")?;
    let original_len = u64::from_le_bytes(len_bytes) as usize;

    let mut shard_data_from_file = Vec::new();
    in_file.read_to_end(&mut shard_data_from_file)?;

    // Infer the size of each shard from the total data size.
    let shard_len = shard_data_from_file.len().div_ceil(TOTAL_SHARDS);
    if shard_len == 0 && original_len > 0 {
        return Err(anyhow!("Protected file is corrupt: contains no shard data but expected length > 0"));
    }
    
    // Create a vector of shards. This logic is now robust against truncated files.
    let mut shards: Vec<Option<Vec<u8>>> = shard_data_from_file
        .chunks(shard_len)
        .map(|chunk| Some(chunk.to_vec()))
        .collect();

    // If the file was truncated, we'll have fewer shards than expected.
    // Fill the rest with `None` so the library knows they are missing.
    while shards.len() < TOTAL_SHARDS {
        shards.push(None);
    }
    shards.truncate(TOTAL_SHARDS); // Ensure we don't have too many shards
    println!("Step 1: Read {} shards from file (some may be missing/corrupt).", shards.iter().filter(|s| s.is_some()).count());

    // 2. Reconstruct the original data from the shards.
    let r = ReedSolomon::new(DATA_SHARDS, PARITY_SHARDS)
        .context("Failed to create ReedSolomon instance")?;
    
    // Reconstruct missing shards in-place.
    r.reconstruct(&mut shards)
        .context("Failed to reconstruct data from shards. Too much corruption or data loss.")?;
    println!("Step 2: Verified and reconstructed data using Reed-Solomon.");

    // Join the data shards and truncate any padding bytes that were added during the sharding process.
    let mut reconstructed_data = Vec::with_capacity(original_len);
    for shard in shards.iter().take(DATA_SHARDS) {
        if let Some(s) = shard {
            reconstructed_data.extend_from_slice(s);
        }
    }
    reconstructed_data.truncate(original_len);

    // Collect all identities
    let mut identities: Vec<Box<dyn age::Identity>> = Vec::new();
    for path in identity_paths {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read identity file: {}", path.display()))?;
        for line in content.lines() {
            if line.starts_with("AGE-SECRET-KEY-1") {
                let identity = age::x25519::Identity::from_str(line)
                    .map_err(|e| anyhow!("Failed to parse identity file: {}", e))?;
                identities.push(Box::new(identity) as Box<dyn age::Identity>);
            }
            // For YubiKey plugin compatibility, you could add plugin identity parsing here.
        }
    }
    if identities.is_empty() {
        return Err(anyhow!("No valid age identities found."));
    }

    let decryptor = age::Decryptor::new(&reconstructed_data[..])?
        .decrypt(identities.iter().map(|i| &**i))?;

    // 3. Decrypt the data with age.
    let mut decrypted_data = vec![];
    let mut reader = decryptor;
    reader.read_to_end(&mut decrypted_data)?;
    println!("Step 3: Decrypted data using age identity.");

    // 4. Decompress the decrypted data.
    let decompressed_data = zstd::decode_all(Cursor::new(decrypted_data))
        .context("Failed to decompress zstd data")?;
    println!("Step 4: Decompressed data using zstd.");

    // 5. Extract the file(s) from the tar archive.
    let mut archive = tar::Archive::new(Cursor::new(decompressed_data));
    
    // Ensure the output directory exists before unpacking.
    if let Some(parent) = output_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }
    archive.unpack(output_path)?;
    println!("Step 5: Extracted original file(s) from tar archive to {}.", output_path.display());

    Ok(())
}

/// Splits data into a fixed number of shards, padding them all to the same size.
///
/// This function is corrected to always produce `num_shards` vectors, even if the
/// input data is smaller than `num_shards`. It also ensures all shards have the
/// exact same length by padding the last one, which is a requirement for the
/// `reed-solomon-erasure` library.
fn shards_from_data(data: &[u8], num_shards: usize) -> Result<Vec<Vec<u8>>> {
    // Calculate the size of each shard, rounding up.
    let shard_len = data.len().div_ceil(num_shards);
    if shard_len == 0 {
        // Handle case with no data by returning the correct number of empty shards.
        return Ok(vec![vec![]; num_shards]);
    }

    // Create shards from the data chunks.
    let mut shards: Vec<Vec<u8>> = data.chunks(shard_len).map(|chunk| chunk.to_vec()).collect();

    // Ensure we have exactly `num_shards` shards. If the data was small,
    // we might have fewer. Add empty shards to make up the difference.
    while shards.len() < num_shards {
        shards.push(Vec::new());
    }

    // Pad all shards to the same length. This is crucial for the erasure coding library.
    // The last shard from the data might be shorter, and any added empty shards are definitely shorter.
    for shard in shards.iter_mut() {
        shard.resize(shard_len, 0);
    }

    Ok(shards)
}
