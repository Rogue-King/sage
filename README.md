# Sage

Sage is a command-line tool to compress, encrypt, and add error correction to files or directories. It uses [age](https://github.com/FiloSottile/age) for encryption and [zstd](https://facebook.github.io/zstd/) for compression, archiving files into a tarball before encrypting.

## Features

- **Protect (Encrypt):** Compresses and encrypts files or directories for secure storage or transfer.
- **Recover (Decrypt):** Decrypts and extracts protected archives.
- **Multiple Recipients:** Supports encrypting to multiple recipients or recipient files.
- **Identity Files:** Supports multiple identity files for decryption.
- **Configurable Compression:** Set zstd compression level (1-22, default: 3).
- **Debug Logging:** Enable debug output for troubleshooting.

## Usage

```sh
sage --encrypt --input <INPUT> --output <OUTPUT> [--recipient <RECIPIENT> ...] [--recipients-file <FILE> ...] [--identity-file <IDENTITY> ...] [--compression-level <LEVEL>] [--debug]
sage --decrypt --input <INPUT> --output <OUTPUT> [--identity-file <IDENTITY> ...] [--debug]
```

### Options

- `-e`, `--encrypt` : Encrypt (protect) the input (mutually exclusive with `--decrypt`)
- `-d`, `--decrypt` : Decrypt (recover) the input (mutually exclusive with `--encrypt`)
- `--input <INPUT>` : Path to the input file or directory (required)
- `-o`, `--output <OUTPUT>` : Path for the output file (required)
- `-r`, `--recipient <RECIPIENT>` : Encrypt to the specified recipient (can be repeated)
- `-R`, `--recipients-file <FILE>` : Encrypt to recipients listed at path (can be repeated)
- `-i`, `--identity-file <IDENTITY>` : Path to the identity file (can be repeated)
- `-c`, `--compression-level <LEVEL>` : Set zstd compression level (1-22, default: 3)
- `--debug` : Enable debug logging

## Example

Encrypt a directory for a recipient:

```sh
sage --encrypt --input my_folder --output my_folder.sage --recipient age1example...
```

Encrypt a file with custom compression and debug logging:

```sh
sage --encrypt --input notes.txt --output notes.sage --recipient age1example... --compression-level 10 --debug
```

Decrypt an archive:

```sh
sage --decrypt --input my_folder.sage --output ./restored_folder --identity-file key.txt
```

## Building

This project uses Rust. To build:

```sh
cargo build --release
```

## License

MIT or Apache-2.0