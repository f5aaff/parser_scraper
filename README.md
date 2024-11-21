# Parser_scraper

this is a simple cli tool for grabbing tree-sitter parsers, via scraping [this](https://github.com/tree-sitter/tree-sitter/wiki/List-of-parsers) wiki page.


# Requirements
- git
- gcc
- openssl
- openssl-devel

# building
- building this should be as simple as running:
```cargo build```

# Usage
```Usage: parser_scraper [OPTIONS]

Options:
  -o, --output <OUTPUT>                          [default: ./shared_libs/]
  -s, --source-destination <SOURCE_DESTINATION>  [default: ./shared_libs_src/]
  -t, --threads <THREADS>                        [default: 10]
  -l, --languages <LANGUAGES>
  -h, --help                                     Print help
  -V, --version
```

- ```./parser_scraper```
+   this will attempt to clone and build every parser in the list, which is ~400. this might take a while.

- ```./parser_scraper -l python,go,rust,java```
+ using the -l(languages) flag, will only attempt to clone and build parsers matching those languages.

- ```./parser_scraper -t 50```
+ parser_scraper generates a thread per repo, this limits the max number of concurrent threads it will use,
in this case; 50.
