# LUnpack

Command line tool to unpack LU clients.

## Usage

```console
$ lunpack [path/to/packed] [-o path/to/unpacked] [-d] [-g globs.txt]
```

If you're on windows, use `lunpack.exe`

## Install

If you have [Rust](https://rust-lang.org) installed, you can use `cargo install --git https://github.com/Xiphoseer/LUnpack.git`.

Otherwise, you can download a release from <https://github.com/Xiphoseer/LUnpack/releases>.

## Options

- *input*: `path/to/packed` (optional)  
  - Specify a directory that contains at least a `versions` (downloaddir in patcher-terms) and a `client` folder.
  - Default value: The current directory
- *output*: `-o path/to/unpacked` (optional)
  - Specifiy a directory to place the unpacked files into.
  - Default value: The *input* directory
- *dry run*: `-d`
  - Print a list of files instead of extracting them
- *globset*: `-g globs.txt`
  - Instead of all files, use only the files that match at least one line in the specified file

## Globset for Darkflame Universe (DLU)

Use the following file as `globs.txt` to unpack just the necessary files for running a DLU server.

The name of the file doesn't matter as long as you pass the same path to the `-g` flag

```console
client/res/macros/**
client/res/BrickModels/**
client/res/maps/**
*.fdb
```