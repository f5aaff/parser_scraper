use anyhow::Error;
use clap::Parser;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use log::{log, LevelFilter};
use log4rs::append::file::FileAppender;
use log4rs::config::{Appender, Config, Root};
use log4rs::encode::pattern::PatternEncoder;
use reqwest::blocking::Client;
use scraper::{Html, Selector};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Write};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::{fs, thread, time::Duration};
use threadpool::ThreadPool;

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    // config file path
    #[arg(short, long, default_value = "./shared_libs/")]
    output: String,

    #[arg(short, long, default_value = "./shared_libs_src/")]
    source_destination: String,

    #[arg(short, long, default_value = "./config.json")]
    config_destination: String,

    // target dir/file
    #[arg(short, long, default_value = "10")]
    threads: usize,

    #[arg(short, long, value_delimiter = ',', required = false)]
    languages: Vec<String>,
}

fn main() {
    // logging ------------------------------------------------------------------
    let logfile = FileAppender::builder()
        .encoder(Box::new(PatternEncoder::new(
            "{l} [{d(%Y-%m-%d %H:%M:%S)}] - {m}\n",
        )))
        .build("log/output.log")
        .unwrap();

    let config = Config::builder()
        .appender(Appender::builder().build("logfile", Box::new(logfile)))
        .build(Root::builder().appender("logfile").build(LevelFilter::Info))
        .unwrap();

    log4rs::init_config(config).unwrap();

    // --------------------------------------------------------------------------
    let url = "https://github.com/tree-sitter/tree-sitter/wiki/List-of-parsers";

    let args = Args::parse();
    let max_threads = args.threads;
    let output_dir = Arc::new(Mutex::new(args.output));
    let source_destination = Arc::new(Mutex::new(args.source_destination));
    let config_destination = Arc::new(Mutex::new(args.config_destination));
    let languages = args.languages;
    let pool = ThreadPool::new(max_threads); // Thread pool with fixed size
    let target_parsers: HashSet<&str> = languages.iter().map(|s| s.as_str()).collect();

    // Step 1: Scrape the list of parsers
    let raw_parsers = match scrape_parsers(url) {
        Ok(rp) => rp,
        Err(e) => {
            eprintln!("Error scraping parsers: {}", e);
            std::process::exit(1);
        }
    };
    let parsers: Vec<(String, String)>;
    if target_parsers.len() > 0 {
        parsers = raw_parsers
            .into_iter()
            .filter(|(lang, _)| target_parsers.contains(lang.as_str()))
            .collect();
    } else {
        parsers = raw_parsers.into_iter().collect();
    }

    let total_parsers = parsers.len();
    let completed = Arc::new(Mutex::new(0)); // Shared counter for progress
    let failed = Arc::new(Mutex::new(0));
    // Step 2: Set up multi-progress bar
    let multi_progress = Arc::new(MultiProgress::new());
    let overall_progress = multi_progress.add(ProgressBar::new(total_parsers as u64));
    overall_progress.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {pos}/{len} completed {msg}")
            .unwrap(),
    );

    // Submit tasks to the thread pool
    for (lang, repo_url) in parsers {
        let completed = Arc::clone(&completed);
        let failed = Arc::clone(&failed);
        let multi_progress = Arc::clone(&multi_progress);
        let overall_progress = overall_progress.clone();
        let output = Arc::clone(&output_dir);
        let source_dest = Arc::clone(&source_destination);
        let config_dest = Arc::clone(&config_destination);
        pool.execute(move || {
            // Create a progress bar only when the task starts
            let pb = multi_progress.add(ProgressBar::new_spinner());
            pb.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.green}[{elapsed_precise}] {msg}")
                    .unwrap(),
            );
            pb.set_message(format!("Cloning {}", lang));
            let pb_clone = pb.clone();
            let spinner_thread = thread::spawn(move || {
                while !pb_clone.is_finished() {
                    pb_clone.tick();
                    thread::sleep(Duration::from_millis(100));
                }
            });

            // Execute the task
            if let Err(e) = clone_and_build(&lang, &repo_url, &pb, output, source_dest,config_dest) {
                pb.finish_with_message(format!("Failed for {}: {}", lang, e));
                log::warn!("failed for {} : {}", lang, e);
                let mut failed_lock = failed.lock().unwrap();
                *failed_lock += 1;
            } else {
                pb.finish_with_message(format!("Done with {}", lang));
                log::info!("Done with {}", lang);
            }

            spinner_thread.join().unwrap();
            // Clean up the progress bar
            multi_progress.remove(&pb);

            // Update overall progress
            let mut completed_lock = completed.lock().unwrap();
            *completed_lock += 1;
            let failed_count = failed.lock().unwrap();
            overall_progress.set_message(format!("{} failed", *failed_count));
            overall_progress.inc(1);
        });
    }

    // Wait for all tasks to finish
    pool.join();
    let failed_count = failed.lock().unwrap();
    overall_progress.finish_with_message(format!("All tasks completed. {} failed.", failed_count));
}

// Scrape parsers from the Tree-sitter wiki
fn scrape_parsers(url: &str) -> Result<HashSet<(String, String)>, Box<dyn std::error::Error>> {
    let client = Client::new();
    let res = client.get(url).send()?.text()?;

    let document = Html::parse_document(&res);
    let container_selector = Selector::parse("div.markdown-body li").unwrap();
    let link_selector = Selector::parse("a").unwrap();

    let mut parsers = HashSet::new();
    for li_element in document.select(&container_selector) {
        if let Some(a_element) = li_element.select(&link_selector).next() {
            if let Some(href) = a_element.value().attr("href") {
                parsers.insert((
                    a_element.text().next().unwrap().to_string(),
                    href.to_string(),
                ));
            }
        }
    }

    Ok(parsers)
}

// Clone and build the grammar for a given language
fn clone_and_build(
    lang: &str,
    repo_url: &str,
    pb: &ProgressBar,
    output_dir: Arc<Mutex<String>>,
    source_destination: Arc<Mutex<String>>,
    config_path: Arc<Mutex<String>>
) -> Result<(), Box<dyn std::error::Error>> {
    pb.set_message(format!("Cloning {}", repo_url));

    let source_destination = source_destination.lock().unwrap();
    // Clone the repository
    let clone_output = Command::new("git")
        .arg("clone")
        .arg(repo_url)
        .arg(format!("{}tree-sitter-{}", source_destination, lang))
        .output()?;

    if !clone_output.status.success() {
        return Err(format!(
            "Failed to clone {}: {}",
            repo_url,
            String::from_utf8_lossy(&clone_output.stderr)
        )
        .into());
    }

    let repo_dir = format!("{}tree-sitter-{}", source_destination, lang);
    pb.set_message(format!("Cloned {}. Searching for parser.c", lang));

    // Search for parser.c in the cloned directory
    let parser_c_path = find_file(&repo_dir, "parser.c")?;
    let scanner_c_path = find_file(&repo_dir, "scanner.c").ok(); // scanner.c is optional
    pb.set_message(format!("Building grammar for {}", lang));
    let output_dir = output_dir.lock().unwrap();
    let output_path = format!("{}lib{}.so",*output_dir,lang);
    // Build the grammar using GCC
    let mut gcc_cmd = Command::new("gcc");
    gcc_cmd
        .arg("-shared")
        .arg("-fPIC")
        .arg("-o")
        .arg(output_path.clone())
        .arg(parser_c_path);

    if let Some(scanner_c) = scanner_c_path {
        gcc_cmd.arg(scanner_c);
    }

    let gcc_output = gcc_cmd.output()?;
    if !gcc_output.status.success() {
        return Err(format!(
            "Failed to build grammar for {}: {}",
            lang,
            String::from_utf8_lossy(&gcc_output.stderr)
        )
        .into());
    }

    let config_path = config_path.lock().unwrap();

    match create_config_entry(&repo_dir, &config_path, &output_path){
        Ok(()) => (),
        Err(e) => {
            log::error!("failed to create config entry for {} : {}",lang,e);
        }
    };
    pb.set_message(format!("Built grammar for {}", lang));
    Ok(())
}

fn create_config_entry(
    repo_dir: &str,
    config_path: &str,
    shared_object_path: &str
) -> Result<(), Box<dyn std::error::Error>> {
    // read the tree-sitter.json from the target repo
    let json_path = find_file(repo_dir, "tree-sitter.json")?;
    let mut file = File::open(json_path)?;
    let mut file_content = String::new();
    file.read_to_string(&mut file_content)?;

    let tree_sitter_json: Value = serde_json::from_str(&file_content)?;

    // read the config file (existing known_languages data) or initialize a new structure
    let mut known_languages = if let Ok(mut output_file) = File::open(config_path) {
        let mut output_file_content = String::new();
        output_file.read_to_string(&mut output_file_content)?;
        let existing_json: Value = serde_json::from_str(&output_file_content)?;
        existing_json
            .get("known_languages")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    } else {
        Map::new() // Start fresh if the output file doesn't exist
    };

    if let Some(grammars) = tree_sitter_json.get("grammars").and_then(Value::as_array) {
        for grammar in grammars {
            if let Some(name) = grammar.get("name").and_then(Value::as_str) {
                let extension = grammar
                    .get("file-types")
                    .and_then(Value::as_array)
                    .and_then(|arr| arr.first())
                    .and_then(Value::as_str)
                    .unwrap_or(""); // Default to empty if no extension found

                // Add or update the entry in known_languages
                known_languages.insert(
                    name.to_string(),
                    json!({
                        "path": shared_object_path,
                        "extension": extension
                    }),
                );
            }
        }
    }

    let output_json = json!({
        "known_languages": known_languages
    });

    let mut output_file = File::create(config_path)?;
    output_file.write_all(output_json.to_string().as_bytes())?;

    Ok(())
}

fn find_file(dir: &str, filename: &str) -> Result<String, Box<dyn std::error::Error>> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() && path.file_name().unwrap_or_default() == filename {
            return Ok(path.to_string_lossy().to_string());
        } else if path.is_dir() {
            // Recursive search in the subdirectory
            if let Ok(found_path) = find_file(&path.to_string_lossy(), filename) {
                return Ok(found_path);
            }
        }
    }
    Err(format!("File {} not found in {}", filename, dir).into())
}
