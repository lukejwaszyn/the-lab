//! `thelab`: the command-line interface and MCP server for The Lab.
//!
//! From github.com/lukejwaszyn/the-lab-native (MIT, Luke Waszyn).

mod mcp;
mod report;

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;
use thelab_core::compile::{CompileOptions, Engine, compile};
use thelab_core::{bib, check, outline, overview, python, scan, synctex};

const USAGE: &str = "\
thelab: LaTeX project tools and an MCP server for Claude Code

Usage: thelab <command> [options] [PROJECT]

Commands:
  compile   Compile the project and report errors and warnings
  outline   Show the main file, the outline and word counts
  check     Check references and citations without compiling
  bib       List the bibliography, or search it with --query
  synctex   Map a source line to its place in the PDF (--file, --line),
            or a place in the PDF to its source line (--page, --x, --y)
  python    Show which Python runs each folder of the project
  mcp       Run the MCP server on stdin/stdout

Options for mcp:
  --tools LIST    Offer only these tool groups, comma-separated: latex,
                  notebook, sysml, editor (default: all)

Options for the other commands:
  --main FILE     Main .tex file, relative to the project (default: detected)
  --engine NAME   tectonic or latexmk (compile only; default: tectonic, or latexmk
                  for biber and LuaLaTeX documents, if installed)
  --query TEXT    Author, title words, year or key to search for (bib only)
  --file FILE     Source file, relative to the project (synctex only)
  --line N        Line of that file, from 1 (synctex only)
  --page N        Page of the PDF, from 1 (synctex only)
  --x X --y Y     Point on that page, in PDF points from its top-left
                  corner (synctex only)
  --json          Print the result as JSON
  -h, --help      Show this help
  -V, --version   Show the version

PROJECT is the project folder and defaults to the current directory.
Exit status: 0 on success, 1 if the document has problems, 2 if the command
could not run.";

#[derive(Default)]
struct Arguments {
    root: Option<PathBuf>,
    main: Option<String>,
    engine: Option<String>,
    query: Option<String>,
    file: Option<String>,
    line: Option<u32>,
    page: Option<u32>,
    x: Option<f64>,
    y: Option<f64>,
    json: bool,
}

fn parse(arguments: &[String]) -> Result<Arguments, String> {
    let mut parsed = Arguments::default();
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        let mut value = |name: &str| {
            rest.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match argument.as_str() {
            "--json" => parsed.json = true,
            "--main" => parsed.main = Some(value("--main")?),
            "--engine" => parsed.engine = Some(value("--engine")?),
            "--query" => parsed.query = Some(value("--query")?),
            "--file" => parsed.file = Some(value("--file")?),
            "--line" => parsed.line = Some(number(value("--line")?, "--line")?),
            "--page" => parsed.page = Some(number(value("--page")?, "--page")?),
            "--x" => parsed.x = Some(number(value("--x")?, "--x")?),
            "--y" => parsed.y = Some(number(value("--y")?, "--y")?),
            option if option.starts_with('-') => return Err(format!("unknown option {option}")),
            path if parsed.root.is_none() => parsed.root = Some(PathBuf::from(path)),
            extra => return Err(format!("unexpected argument {extra}")),
        }
    }
    Ok(parsed)
}

fn number<T: std::str::FromStr>(text: String, option: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("{option} needs a number, not \"{text}\""))
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = arguments.first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match command.as_str() {
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        "-V" | "--version" => {
            println!("thelab {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        "mcp" => {
            let requested = match mcp_groups(&arguments[1..]) {
                Ok(requested) => requested,
                Err(message) => {
                    eprintln!("thelab mcp: {message}");
                    return ExitCode::from(2);
                }
            };
            for group in requested.iter().flat_map(|groups| groups.empty_ones()) {
                // Accepted, so that a configuration written today keeps
                // working when the group gets its tools. stderr, not stdout.
                eprintln!(
                    "thelab mcp: the {} group has no tools in this version",
                    group.name()
                );
            }
            let groups = requested.unwrap_or_else(mcp::Groups::all);
            return match mcp::serve(std::io::stdin().lock(), std::io::stdout().lock(), groups) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("thelab mcp: {error}");
                    ExitCode::from(2)
                }
            };
        }
        _ => {}
    }

    let outcome = parse(&arguments[1..]).and_then(|parsed| run(command, parsed));
    match outcome {
        Ok(clean) => ExitCode::from(if clean { 0 } else { 1 }),
        Err(message) => {
            eprintln!("thelab: {message}");
            ExitCode::from(2)
        }
    }
}

/// Reads `thelab mcp`'s own options: `--tools a,b` or `--tools=a,b`.
/// `None` means no groups were named, which selects them all.
fn mcp_groups(arguments: &[String]) -> Result<Option<mcp::Groups>, String> {
    let mut groups = None;
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        let list = match argument.strip_prefix("--tools") {
            Some("") => rest.next().ok_or("--tools needs a value")?.as_str(),
            Some(value) if value.starts_with('=') => &value[1..],
            _ => return Err(format!("unexpected argument {argument}")),
        };
        groups = Some(mcp::Groups::parse(list)?);
    }
    Ok(groups)
}

/// Runs a command. `Ok(false)` means it ran and found problems.
fn run(command: &str, arguments: Arguments) -> Result<bool, String> {
    let root = arguments.root.clone().unwrap_or_else(|| PathBuf::from("."));
    let fail = |error: thelab_core::Error| error.to_string();
    let print = |json: serde_json::Value, text: String| {
        if arguments.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json).expect("results are valid JSON")
            );
        } else {
            println!("{text}");
        }
    };

    match command {
        "compile" => {
            let engine = arguments
                .engine
                .as_deref()
                .map(Engine::parse)
                .transpose()
                .map_err(fail)?;
            let options = CompileOptions {
                main: arguments.main.clone(),
                engine,
            };
            let result = compile(&root, &options).map_err(fail)?;
            print(json!(result), report::compile_text(&result));
            Ok(result.ok)
        }
        "outline" => {
            let project = scan::scan(&root, arguments.main.as_deref()).map_err(fail)?;
            let overview = overview::overview(&project).map_err(fail)?;
            print(json!(overview), report::overview_text(&overview));
            Ok(true)
        }
        "check" => {
            let project = scan::scan(&root, arguments.main.as_deref()).map_err(fail)?;
            let outline = outline::read_outline(&project).map_err(fail)?;
            let report = check::check(&project, &outline).map_err(fail)?;
            print(json!(report), report::check_text(&report));
            Ok(!report.has_problems())
        }
        "bib" => {
            let project = scan::scan(&root, arguments.main.as_deref()).map_err(fail)?;
            let outline = outline::read_outline(&project).map_err(fail)?;
            let entries = check::bibliography(&project, &outline).map_err(fail)?;
            match &arguments.query {
                Some(query) => {
                    let found = bib::search(&entries, query, 10);
                    let matches: Vec<_> = found.matches.iter().map(|m| m.entry).collect();
                    print(
                        json!({ "query": query, "exact": found.exact, "entries": matches }),
                        report::citations_text(query, &found),
                    );
                    Ok(!found.matches.is_empty())
                }
                None => {
                    let lines: Vec<String> = entries
                        .iter()
                        .map(|e| format!("{}: {}", e.key, e.summary()))
                        .collect();
                    print(json!({ "entries": entries }), lines.join("\n"));
                    Ok(true)
                }
            }
        }
        "synctex" => {
            let project = scan::scan(&root, arguments.main.as_deref()).map_err(fail)?;
            let sync = synctex::load(&project).map_err(fail)?;
            match (
                &arguments.file,
                arguments.line,
                arguments.page,
                arguments.x,
                arguments.y,
            ) {
                (Some(file), Some(line), None, None, None) => {
                    // An absolute path inside the project is as good as a relative one.
                    let root_prefix = format!("{}/", project.root.display());
                    let file = file.strip_prefix(&root_prefix).unwrap_or(file);
                    match sync.forward(&project.root, file, line) {
                        Some(found) => {
                            let text =
                                format!("page {} at ({:.1}, {:.1})", found.page, found.x, found.y);
                            let mut json = json!(found);
                            json["found"] = json!(true);
                            print(json, text);
                            Ok(true)
                        }
                        None => {
                            print(
                                json!({ "found": false }),
                                format!("nothing in the PDF comes from {file}"),
                            );
                            Ok(false)
                        }
                    }
                }
                (None, None, Some(page), Some(x), Some(y)) => {
                    match sync.inverse(&project.root, page, x, y) {
                        Some(found) => {
                            let text = format!("{}:{}", found.file, found.line);
                            let mut json = json!(found);
                            json["found"] = json!(true);
                            print(json, text);
                            Ok(true)
                        }
                        None => {
                            print(
                                json!({ "found": false }),
                                "no source line is recorded near that point".to_string(),
                            );
                            Ok(false)
                        }
                    }
                }
                _ => Err("synctex needs --file and --line, or --page, --x and --y".to_string()),
            }
        }
        "python" => {
            let project = scan::scan(&root, None).map_err(fail)?;
            let map = python::python_map(&project);
            print(json!(map), report::python_text(&map));
            Ok(!map.has_problems())
        }
        other => Err(format!("unknown command {other}\n\n{USAGE}")),
    }
}
