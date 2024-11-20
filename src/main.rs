use reqwest::blocking::Client;
use scraper::{Html, Selector};
use std::collections::HashSet;
use std::{fs, thread, time::Duration};
//use std::path::Path;
use clap::Parser;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::process::Command;
use std::sync::{Arc, Mutex};
use threadpool::ThreadPool;

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    // config file path
    #[arg(short, long, default_value = "./shared_libs/")]
    output: String,

    // target dir/file
    #[arg(short, long, default_value = "10")]
    threads: usize,
}
fn main() {
    let url = "https://github.com/tree-sitter/tree-sitter/wiki/List-of-parsers";

    let args = Args::parse();
    let max_threads = args.threads;
    let output_dir = Arc::new(Mutex::new(args.output));
    let pool = ThreadPool::new(max_threads); // Thread pool with fixed size
    let target_parsers: HashSet<&str> = [
        "python",
        "javascript",
        "java",
        "rust",
        "go",
        "cpp",
        "cplusplus",
    ]
    .iter()
    .cloned()
    .collect();
    // Step 1: Scrape the list of parsers
    let raw_parsers = scrape_parsers(url).unwrap();
    let parsers: Vec<_> = raw_parsers
        .into_iter()
        .filter(|(lang, _)| target_parsers.contains(lang.as_str()))
        .collect();
    let total_parsers = parsers.len();
    let completed = Arc::new(Mutex::new(0)); // Shared counter for progress

    // Step 2: Set up multi-progress bar
    let multi_progress = Arc::new(MultiProgress::new());
    let overall_progress = multi_progress.add(ProgressBar::new(total_parsers as u64));
    overall_progress.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {pos}/{len} completed")
            .unwrap(),
    );

    // Submit tasks to the thread pool
    for (lang, repo_url) in parsers {
        let completed = Arc::clone(&completed);
        let multi_progress = Arc::clone(&multi_progress);
        let overall_progress = overall_progress.clone();
        let output = Arc::clone(&output_dir);
        pool.execute(move || {
            // Create a progress bar only when the task starts
            let pb = multi_progress.add(ProgressBar::new_spinner());
            pb.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.green} {msg}")
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
            if let Err(e) = clone_and_build(&lang, &repo_url, &pb,output) {
                pb.set_message(format!("Failed for {}: {}", lang, e));
            } else {
                pb.finish_with_message(format!("Done with {}", lang));
            }

            spinner_thread.join().unwrap();
            // Clean up the progress bar
            multi_progress.remove(&pb);

            // Update overall progress
            let mut completed_lock = completed.lock().unwrap();
            *completed_lock += 1;
            overall_progress.inc(1);
        });
    }

    // Wait for all tasks to finish
    pool.join();
    overall_progress.finish_with_message("All tasks completed.");
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
) -> Result<(), Box<dyn std::error::Error>> {
    pb.set_message(format!("Cloning {}", repo_url));

    // Clone the repository
    let clone_output = Command::new("git")
        .arg("clone")
        .arg(repo_url)
        .arg(format!("tree-sitter-{}", lang))
        .output()?;

    if !clone_output.status.success() {
        return Err(format!(
            "Failed to clone {}: {}",
            repo_url,
            String::from_utf8_lossy(&clone_output.stderr)
        )
        .into());
    }

    let repo_dir = format!("tree-sitter-{}", lang);
    pb.set_message(format!("Cloned {}. Searching for parser.c", lang));

    // Search for parser.c in the cloned directory
    let parser_c_path = find_file(&repo_dir, "parser.c")?;
    let scanner_c_path = find_file(&repo_dir, "scanner.c").ok(); // scanner.c is optional

    pb.set_message(format!("Building grammar for {}", lang));

    let output_dir = output_dir.lock().unwrap();
    // Build the grammar using GCC
    let mut gcc_cmd = Command::new("gcc");
    gcc_cmd
        .arg("-shared")
        .arg("-fPIC")
        .arg("-o")
        .arg(format!("{}lib{}.so",*output_dir, lang))
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

    pb.set_message(format!("Built grammar for {}", lang));
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
