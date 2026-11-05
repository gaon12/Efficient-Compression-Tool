use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::Arc;

use ect::{Config, Report};

struct CliInput {
    cfg: Config,
    paths: Vec<String>,
    show_help: bool,
}

fn parse_cli(args: impl IntoIterator<Item = String>) -> Result<CliInput, String> {
    let mut cfg = Config::default();
    let mut paths = Vec::new();
    let mut positional_only = false;

    for arg in args {
        if positional_only || !arg.starts_with('-') {
            paths.push(arg);
            continue;
        }
        let b = arg.as_bytes();
        match arg.as_str() {
            "--" => positional_only = true,
            _ if b.len() == 2 && (b'1'..=b'9').contains(&b[1]) => {
                cfg.level = u32::from(b[1] - b'0')
            }
            "-strip" => cfg.strip = true,
            "-recurse" => cfg.recursive = true,
            "-quiet" => cfg.quiet = true,
            "-keep" => cfg.keep_mod_time = true,
            "-gzip" => cfg.gzip_mode = true,
            "-zip" => {
                cfg.zip_mode = true;
                cfg.gzip_mode = true;
            }
            "-help" | "--help" | "-h" => {
                return Ok(CliInput {
                    cfg,
                    paths,
                    show_help: true,
                });
            }
            "--disable-png" => cfg.disable_png = true,
            "--disable-jpg" | "--disable-jpeg" => cfg.disable_jpeg = true,
            "--disable-webp" => cfg.disable_webp = true,
            "--strict" => cfg.strict = true,
            "--mt-file" => {}
            _ if arg.starts_with("--mt-file=") => {
                let v = &arg["--mt-file=".len()..];
                match v.parse::<usize>() {
                    Ok(n) if n >= 1 => cfg.workers = n,
                    _ => return Err(format!("invalid --mt-file value: {v}")),
                }
            }
            // Accepted for CLI compatibility with the C/C++ version.
            "--mt-deflate" | "-progressive" | "-autorotate" | "-autorotate=force" | "--reuse"
            | "--allfilters" | "--allfilters-b" | "--allfilters-c" => {}
            _ if arg.starts_with("--mt-deflate=") || arg.starts_with("--pal_sort=") => {}
            _ => return Err(format!("unknown option: {arg}")),
        }
    }
    Ok(CliInput {
        cfg,
        paths,
        show_help: false,
    })
}

fn usage(w: &mut dyn Write) {
    let _ = writeln!(
        w,
        "Efficient Compression Tool (Rust rewrite)
Usage: ect [Options] files/folders...
Options:
 -1 to -9          Set compression level (default: 3)
 -strip            Strip metadata where format allows
 -recurse          Recursively process input directories
 -gzip             Compress non-gzip files to .gz and optimize .gz
 -zip              Create .zip archives for each given path
 -keep             Keep original modification times
 -quiet            Print only error messages
 -help             Show help
 --disable-png     Disable PNG optimization
 --disable-jpg     Disable JPEG optimization
 --disable-webp    Disable WebP optimization
 --strict          Keep stricter behavior for gzip/zip
 --mt-file[=N]     Number of file workers
 --                Treat all following arguments as positional paths"
    );
}

fn format_bytes(mut v: f64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{v:.0}{}", UNITS[i])
    } else {
        format!("{v:.2}{}", UNITS[i])
    }
}

fn print_summary(r: &Report, quiet: bool) {
    if quiet {
        return;
    }
    if r.processed_files == 0 {
        println!("No compatible files found");
        return;
    }
    println!(
        "Processed {} file{}",
        r.processed_files,
        if r.processed_files == 1 { "" } else { "s" }
    );
    if r.total_bytes == 0 {
        println!("No compatible files found");
        return;
    }
    let total = format_bytes(r.total_bytes as f64);
    let pct = 100.0 * r.saved_bytes as f64 / r.total_bytes as f64;
    if r.saved_bytes < 0 {
        println!(
            "Result is bigger by {} out of {total} ({pct:.4}%)",
            format_bytes(-r.saved_bytes as f64)
        );
    } else {
        println!(
            "Saved {} out of {total} ({pct:.4}%)",
            format_bytes(r.saved_bytes as f64)
        );
    }
}

fn main() -> ExitCode {
    let parsed = match parse_cli(std::env::args().skip(1)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            usage(&mut io::stderr());
            return ExitCode::from(2);
        }
    };
    if parsed.show_help {
        usage(&mut io::stdout());
        return ExitCode::SUCCESS;
    }
    if parsed.paths.is_empty() {
        usage(&mut io::stdout());
        return ExitCode::from(1);
    }

    let mut cfg = parsed.cfg;
    cfg.log = Some(Arc::new(|msg: &str| println!("{msg}")));
    let quiet = cfg.quiet;

    let report = ect::process(cfg, &parsed.paths);
    for err in &report.errors {
        eprintln!("{}: {}", err.path.display(), err.error);
    }
    print_summary(&report, quiet);

    if report.has_errors() {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<CliInput, String> {
        parse_cli(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_flags_and_paths() {
        let p = parse(&["-9", "-strip", "--mt-file=3", "a.png", "--", "-weird"]).unwrap();
        assert_eq!(p.cfg.level, 9);
        assert!(p.cfg.strip);
        assert_eq!(p.cfg.workers, 3);
        assert_eq!(p.paths, vec!["a.png", "-weird"]);
    }

    #[test]
    fn zip_implies_gzip() {
        let p = parse(&["-zip", "x"]).unwrap();
        assert!(p.cfg.zip_mode && p.cfg.gzip_mode);
    }

    #[test]
    fn rejects_unknown() {
        assert!(parse(&["--nope"]).is_err());
        assert!(parse(&["--mt-file=0"]).is_err());
    }
}
