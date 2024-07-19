#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_imports)]

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use clap::{Parser, Subcommand};
use reqwest;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::io::prelude::*;
use std::path::PathBuf;
use users::{get_current_uid, get_user_by_uid};

pub const DEFAULT_CONFIG: &str = include_str!("../config.yaml");
pub const DEFAULT_CONFIG_FILE_PATH: &str = "~/.config/bookmarks/config.yaml";
pub const USER_AGENT_STRING: &str =
  "Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";

/// A bookmark manager
#[derive(Debug, Parser)]
#[command(
  name = format!("{} ({} {})", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"), env!("GIT_HASH")),
  version = "",
  max_term_width = 100,
)]
struct Opt {
  /// YAML configuration file to use.
  /// If not provided, use the file ~/.config/bookmark/config.yaml.
  /// If the file does not exists, use default embedded config (see --print-config)
  #[arg(short, long, value_name = "FILE", verbatim_doc_comment)]
  config: Option<PathBuf>,
  /// Print the used configuration and exit.
  /// You can use this option to initialize the default config file with:
  ///   mkdir -p ~/.config/bookmark/
  ///   bookmark --print-config ~/.config/bookmark/config.yaml
  #[arg(long, verbatim_doc_comment)]
  print_config: bool,
  /// Override the configured bookmark file
  #[arg(short, long, value_name = "FILE")]
  bookmarks: Option<String>,

  #[command(subcommand)]
  command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
  /// Adds a bookmark
  Add { url: String },
}

#[derive(Debug, Serialize, Deserialize)]
struct Metadata {
  // posted: Option<DateTime<Utc>>,
  posted: Option<NaiveDateTime>,
  user: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Bookmark {
  href: String,
  meta: Metadata,
  title: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Config {
  bookmarks: PathBuf,
}

fn add(bookmarks: &mut Vec<Bookmark>, url: &str, output_file: &PathBuf) -> Result<()> {
  // Check the url is not already present
  if let Some(result) = bookmarks.iter().find(|b| b.href == *url) {
    eprint!(
      "warning: this url is already present in bookmarks: {}",
      result.title
    );
    if let Some(date) = result.meta.posted {
      eprint!(" added the {}", date);
    }
    eprintln!("");
    Ok(())
  } else {
    print!("fetching {}... ", url);
    let _ = std::io::stdout().flush();
    // Creating a client with a standard USER_AGENT because some site do not accetp "reqwests".
    let client = reqwest::blocking::Client::new();
    let response = client
      .get(url)
      .header(reqwest::header::USER_AGENT, USER_AGENT_STRING)
      .send()?;
    // Check for status
    let response = match response.error_for_status() {
      Ok(response) => response,
      Err(e) => Err(e)?,
    };
    // Extract content
    let body = response.text()?;
    let document = Html::parse_document(&body);
    let selector = Selector::parse(r#"title"#).unwrap();
    // Get the title
    if let Some(title_element) = document.select(&selector).next() {
      if let Some(title) = title_element.text().next() {
        let user = get_user_by_uid(get_current_uid()).unwrap();
        // Create the new bookmark and add it to the list
        bookmarks.push(Bookmark {
          href: url.to_string(),
          title: title.to_string(),
          meta: Metadata {
            posted: Some(chrono::offset::Utc::now().naive_utc()),
            user: Some(user.name().to_string_lossy().to_string()),
          },
        });
        // Write the bookmark file
        let file = std::fs::File::create(output_file)?;
        let mut writer = std::io::BufWriter::new(file);
        serde_json::to_writer(&mut writer, &bookmarks)?;
        println!("\radded {}", title);
        Ok(())
      } else {
        Err(anyhow::anyhow!(
          "could not retrieve text from title in the html page"
        ))
      }
    } else {
      Err(anyhow::anyhow!(
        "could not retrieve title from the html page"
      ))
    }
  }
}

fn main() -> Result<(), Box<dyn Error>> {
  let opt = Opt::parse();
  // Load the config. We first check if a config file was provided as an option
  let config_file = if let Some(config_file) = opt.config {
    // Try to load it
    match std::fs::read_to_string(&config_file) {
      Err(e) => {
        eprintln!("error: {e}: {}", config_file.display());
        std::process::exit(1);
      }
      Ok(config) => config,
    }
  } else {
    // Otherwise, try the standard path
    if std::fs::metadata(std::path::Path::new(DEFAULT_CONFIG_FILE_PATH)).is_ok() {
      // Try to load it
      match std::fs::read_to_string(&DEFAULT_CONFIG_FILE_PATH) {
        Err(e) => {
          eprintln!("error: {e}: {}", DEFAULT_CONFIG_FILE_PATH);
          std::process::exit(1);
        }
        Ok(config) => config,
      }
    } else {
      // Otherwise, just use the embedded config file
      DEFAULT_CONFIG.to_string()
    }
  };

  if opt.print_config {
    // Print the content of the configuration file and exit
    println!("{}", config_file);
    return Ok(());
  }

  let mut config: Config = serde_yaml::from_str(&config_file)?;
  if let Some(bookmarks) = opt.bookmarks {
    config.bookmarks = std::path::PathBuf::from(&bookmarks);
  }

  // Load the bookmark files
  let mut bookmarks: Vec<Bookmark> = {
    let inputfile = match std::fs::File::open(&config.bookmarks) {
      Ok(inputfile) => inputfile,
      Err(e) => {
        eprintln!("{}: {}", config.bookmarks.display(), e);
        std::process::exit(1);
      }
    };
    match serde_json::from_reader(std::io::BufReader::new(inputfile)) {
      Ok(json) => json,
      Err(e) => {
        eprintln!(
          "{}: could not parse json file: {}",
          config.bookmarks.display(),
          e
        );
        std::process::exit(1);
      }
    }
  };

  match &opt.command {
    Some(Commands::Add { url }) => add(&mut bookmarks, &url, &config.bookmarks)?,
    None => {
      for i in 0..bookmarks.len() {
        println!("{} {} ({})", i, bookmarks[i].title, bookmarks[i].href);
      }
      println!("{} bookmarks", bookmarks.len());
    }
  }

  Ok(())
}
