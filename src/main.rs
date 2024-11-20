use reqwest::blocking::Client;
use scraper::{Html, Selector};
use std::{collections::HashSet, process::Command};

fn main() {
    let url = "https://github.com/tree-sitter/tree-sitter/wiki/List-of-parsers";

    // Step 1: Scrape the list of parsers from the Tree-sitter wiki.
    let parsers = scrape_parsers(url).unwrap();

    // Step 2: Run the Bash script for each parser.
    for pair in parsers {
        println!("lang:{}", pair.0);
        if let Err(e) = run_bash_script(&pair.0, &pair.1,"shared_libs/") {
            eprintln!("Failed to process {}: {}", &pair.0, e);
        }
    }
}

// Scrape the list of parsers from the Tree-sitter wiki
fn scrape_parsers(url: &str) -> Result<HashSet<(String, String)>, Box<dyn std::error::Error>> {
    let client = Client::new();
    let res = client.get(url).send()?.text()?;

    // Parse the HTML page.
    let document = Html::parse_document(&res);

    // Target <li> elements inside <div class="markdown-body">
    let container_selector = Selector::parse("div.markdown-body li").unwrap();
    let link_selector = Selector::parse("a").unwrap();

    // Extract the language names from the <a> tags within <li> elements.
    let mut parsers: HashSet<(String, String)> = HashSet::new();
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

// Run the bash script for each language parser.
fn run_bash_script(lang: &str, repo_url: &str, destination: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("Cloning {}...", repo_url);

    let clone_output = Command::new("git").arg("clone").arg(&repo_url).output()?;

    if !clone_output.status.success() {
        return Err(format!(
            "Failed to clone {}: {}",
            repo_url,
            String::from_utf8_lossy(&clone_output.stderr)
        )
        .into());
    }

    println!("Building {} grammar...", lang);

    let tree_sitter_cli = Command::new("tree-sitter")
        .arg("generate")
        .arg("--libdir")
        .arg(format!("tree-sitter-{}", lang))
        .arg(format!("tree-sitter-{}/grammar.js", lang))
        .output()?;
    if !tree_sitter_cli.status.success() {
        println!(
            "tree-sitter-cli failed to generate project files for {}: {}",
            lang,
            String::from_utf8_lossy(&tree_sitter_cli.stderr)
        );
    }

    let build_output = Command::new("gcc")
        .arg("-shared")
        .arg("-fPIC")
        .arg("-o")
        .arg(format!("{}lib{}.so", destination,lang))
        .arg(format!("tree-sitter-{}/src/parser.c", lang))
        .output()?;

    if !build_output.status.success() {
        return Err(format!(
            "Failed to build {} grammar: {}",
            lang,
            String::from_utf8_lossy(&build_output.stderr)
        )
        .into());
    }

    println!("Successfully built {} grammar.", lang);
    Ok(())
}
