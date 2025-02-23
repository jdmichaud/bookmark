// echo [] > test.json && cargo run -- --bookmarks test.json add https://news.ycombinator.com/item?id=41074703
#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_imports)]

use anyhow::{Error as E, Context, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use clap::{Parser, Subcommand};
use reqwest;
use scraper::{Html, Selector};
use serde::Deserializer;
use serde::{Deserialize, Serialize};
use std::env;
use std::error::Error;
use std::io::prelude::*;
use std::path::PathBuf;
use users::{get_current_uid, get_user_by_uid};

pub const DEFAULT_CONFIG: &str = include_str!("../config.yaml");
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
  /// Search the needle among the articles
  Search { needle: Vec<String> },
  /// temporary
  Fetch { urls: Vec<String> },
  /// Print the url associated with the provided hash if present in the bookmark file
  Hash { hash: String },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Metadata {
  #[serde(skip_serializing_if = "Option::is_none")]
  posted: Option<NaiveDateTime>,
  #[serde(skip_serializing_if = "Option::is_none")]
  user: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  referer: Option<String>,
}

// This is the representation of Bookmark when serialize
#[derive(Debug, Serialize, Deserialize, Clone)]
struct BookmarkRepr {
  href: String,
  meta: Metadata,
  title: String,
}

// This is a bookmark with computed fields (hash)
// as described in https://github.com/serde-rs/serde/issues/1689#issuecomment-653831474
#[derive(Debug, Serialize, Clone)]
struct Bookmark {
  href: String,
  #[serde(skip_serializing)]
  hash: String,
  meta: Metadata,
  title: String,
}

// https://github.com/serde-rs/serde/issues/1689#issuecomment-653831474
impl<'de> Deserialize<'de> for Bookmark {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: Deserializer<'de>,
  {
    let bookmark = BookmarkRepr::deserialize(deserializer)?;
    Ok(Bookmark {
      hash: get_hash(&bookmark.href),
      href: bookmark.href,
      meta: bookmark.meta,
      title: bookmark.title,
    })
  }
}

#[derive(Debug, Serialize, Deserialize)]
struct ChromiumConfig {
  enabled: bool,
  path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Config {
  // Where to load the bookmark file.
  // Default is ~/bookmarks.json
  bookmarks: PathBuf,
  // Keep a local copy of the article
  // Kept in XDG_DATA_HOME
  // default: false
  store_articles: Option<bool>,
  // Enable search feature.
  // Implicitly turn on store_articles feature.
  // default: false
  search: Option<bool>,
  // The config used to launch chromium to retrieve the page content including
  // with javascript enabled.
  chromium: Option<ChromiumConfig>,
}

// Writes bookmarks to a file.
fn write_bookmarks(bookmarks: &Vec<Bookmark>, output_file: &PathBuf) -> Result<()> {
  let file = std::fs::File::create(output_file)?;
  let mut writer = std::io::BufWriter::new(file);
  serde_json::to_writer(&mut writer, &bookmarks)?;
  Ok(())
}

// Takes a bookmarks list and returns the list without duplicate href.
fn dedup(bookmarks: &[Bookmark], output_file: &PathBuf) -> Result<Vec<Bookmark>> {
  let mut new_bookmarks: Vec<Bookmark> = Vec::with_capacity(bookmarks.len());
  for bookmark in bookmarks {
    if let None = new_bookmarks.iter().find(|b| b.href == bookmark.href) {
      new_bookmarks.push(bookmark.clone());
    }
  }
  // Let sort the bookmarks by date while we're at it
  new_bookmarks.sort_by(|a, b| {
    if a.meta.posted.is_none() || b.meta.posted.is_none() {
      a.href.cmp(&b.href)
    } else {
      a.meta.posted.unwrap().cmp(&b.meta.posted.unwrap())
    }
  });
  write_bookmarks(&new_bookmarks, output_file)?;
  Ok(new_bookmarks)
}

// Fetches a URL with a fake user agent.
// Returns the page content.
fn fetch_http(config: &Config, url: &str) -> Result<String> {
  if chromium_available(config) {
    fetch_by_chromium(url)
  } else {
    let client = reqwest::blocking::Client::new();
    // Creating a client with a standard USER_AGENT because some site do not accetp "reqwests".
    let response = client
      .get(url)
      .header(reqwest::header::USER_AGENT, USER_AGENT_STRING)
      .send()?;
    // Check for status
    let response = match response.error_for_status() {
      Ok(response) => response,
      Err(e) => Err(e)?,
    };
    Ok(response.text()?)
  }
}

fn get_text(config: &Config, url: &str) -> Result<String> {
  let body = fetch_http(config, url)?;
  let document = Html::parse_document(&body);
  let selector = Selector::parse(r#"body :not(style)"#).unwrap();
  let text_content = document.select(&selector).next().unwrap();
  Ok(text_content.text().collect::<Vec<_>>().join(""))
}

// Prints the url from the hash
fn hash2url(
  config: &Config,
  bookmarks: &Vec<Bookmark>,
  hash: &str,
) -> Result<()> {
  if let Some(bookmark) = bookmarks.iter().find(|b| b.hash == hash) {
    println!("{} ({})", bookmark.title, bookmark.href);
  } else {
    eprintln!("hash not found {}", hash);
  }
  Ok(())
}

struct UrlStore<'a> {
  data_folder: PathBuf,
  config: &'a Config,
}

fn get_hash(key: &str) -> String {
  use base32::Alphabet;
  use sha1::{Digest, Sha1};

  let mut hasher = Sha1::new();
  hasher.update(key);
  let hash = hasher.finalize();
  base32::encode(Alphabet::Crockford, &hash)
}

impl<'a> UrlStore<'a> {
  fn new(config: &'a Config) -> Result<Self> {
    Ok(UrlStore {
      data_folder: get_data_folder()?,
      config: config,
    })
  }

  pub fn fetch_url(self: &Self, url: &str) -> Result<String> {
    let hash = get_hash(url);
    let mut hashpath = self.data_folder.clone();
    hashpath.push(&(hash.clone() + ".html"));
    println!("look for {}", hashpath.display());
    // Check the presence of the content of the url in the data folder
    let content = std::fs::read_to_string(&hashpath).or_else(|_| {
      let content = fetch_http(self.config, url)?;
      let search_enabled = self.config.search.unwrap_or(false);
      if self.config.store_articles.unwrap_or(false) || search_enabled {
        // Save the content in a file in the data folder
        match std::fs::write(&hashpath, &content) {
          Ok(_) => println!("{} saved", hashpath.display()),
          Err(e) => anyhow::bail!("error writing to {} ({})", &hashpath.to_string_lossy(), e),
        }
        if search_enabled {
          // Compute the embeddings of the file
          // FIXME: get rid of unwrap
          let embeddings = compute_embeddings(&content).unwrap();
          // Convert to an array of f32
          let array: Vec<f32> = embeddings.to_vec1()?;
          // Create the embedding file path
          let mut embedding_path = self.data_folder.clone();
          embedding_path.push(&(hash + ".html.embeddings"));
          // Serialize the array to the file
          let file = std::fs::File::create(embedding_path)?;
          let mut writer = std::io::BufWriter::new(file);
          serde_json::to_writer(&mut writer, &array)?;
        }
      }
      Ok::<std::string::String, anyhow::Error>(content)
    })?;
    Ok(content)
  }
}

fn fetch_urls(
  url_store: &UrlStore,
  urls: &[String],
) -> Result<()> {
  for url in urls {
    println!("fetching {}...", url);
    url_store.fetch_url(&url)?;
  }
  Ok(())
}

fn get_hn_article(url_store: &UrlStore, url: &str) -> Result<(String, scraper::Html)> {
  let body = url_store.fetch_url(url)?;
  let hn_document = Html::parse_document(&body);
  let selector = Selector::parse(r#".titleline > a"#).unwrap();
  if let Some(title_line_element) = hn_document.select(&selector).next() {
    if let Some(article_url) = title_line_element.value().attr("href") {
      let article_body = url_store.fetch_url(article_url)?;
      Ok((article_url.to_string(), Html::parse_document(&article_body)))
    } else {
      Err(anyhow::anyhow!(
        "could not retrieve the article link from the hacker news post"
      ))
    }
  } else {
    Err(anyhow::anyhow!(
      "could not get the article title from the hacker news post"
    ))
  }
}

// Fetch the url (of in case of an HN article the original article) and return
// the article url and the title
fn fetch_article(url_store: &UrlStore, url: &str) -> Result<(String, String)> {
  let _ = std::io::stdout().flush();
  // If the url if from an hacker new post, fetch the original article
  let is_hacker_news = url.contains("news.ycombinator.com/item?id=");
  let (article_url, document) = if is_hacker_news {
    get_hn_article(url_store, url)?
  } else {
    let body = url_store.fetch_url(url)?;
    (url.to_string(), Html::parse_document(&body))
  };
  let selector = Selector::parse(r#"title"#).unwrap();
  // Get the title
  if let Some(title_element) = document.select(&selector).next() {
    if let Some(title) = title_element.text().next() {
      Ok((article_url, title.to_string()))
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

// Adds a bookmark based on a URL
// The function will treat hacker news stories differently as it will consider
// them as referer and the article pointer to as the original submission.
fn add(
  config: &Config,
  url_store: &UrlStore,
  bookmarks: &mut Vec<Bookmark>,
  url: &str,
) -> Result<()> {
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
  } else {
    print!("fetching {}... ", url);
    // The article url will be different from the url if the url is from
    // Hacker News. We will bookmark the article url and only keep the url
    // as a referer
    let (article_url, title) = fetch_article(&url_store, &url)?;
    let is_hacker_news = url != article_url;
    let user = get_user_by_uid(get_current_uid()).unwrap();
    // Create the new bookmark and add it to the list
    bookmarks.push(Bookmark {
      hash: get_hash(&article_url),
      href: article_url,
      title: title.to_string(),
      meta: Metadata {
        posted: Some(chrono::offset::Utc::now().naive_utc()),
        user: Some(user.name().to_string_lossy().to_string()),
        referer: if is_hacker_news {
          Some(url.to_string())
        } else {
          None
        },
      },
    });
    // Write the bookmark file
    write_bookmarks(bookmarks, &config.bookmarks)?;

    print!("\radded {}", title);
    println!("\x1b[0K");
  }
  Ok(())
}

fn get_data_folder() -> Result<PathBuf> {
  let default_config_data_path: String =
    env::var("XDG_DATA_HOME").unwrap_or(env::var("HOME")? + "/.local/share") + "/bookmark/";
  std::fs::create_dir_all(&default_config_data_path)?;
  let path = std::path::PathBuf::from(&default_config_data_path);
  return Ok(path);
}

fn get_state_folder() -> Result<PathBuf> {
  let default_config_state_path: String =
    env::var("XDG_STATE_HOME").unwrap_or(env::var("HOME")? + "/.local/state") + "/bookmark/";
  std::fs::create_dir_all(&default_config_state_path)?;
  let path = std::path::PathBuf::from(&default_config_state_path);
  return Ok(path);
}

// Checks if chromium is available in headless mode with the dump-dom option.
// It takes a while to launch chrome so the info is then recorded is a state file.
// TODO: This whole state machine in buggy and does not take into changes in
// configuration or in the chromium installation. Need to rewrite it at some point.
fn chromium_available(config: &Config) -> bool {
  // First check if chromium is enabled in the config and if yes, is a path is
  // provided
  let chromium_path = if let Some(chromium_config) = &config.chromium {
    if !chromium_config.enabled {
      return false;
    }
    if let Some(chromium_path) = &chromium_config.path {
      &chromium_path
    } else {
      "chromium"
    }
  } else {
    // Chromium is not enabled
    return false;
  };
  // If chromium is enabled, check if we haven't already recorded its state
  if let Ok(mut state_folder) = get_state_folder() {
    state_folder.push(std::path::PathBuf::from("chromium_available"));
    if let Ok(state_file) = std::fs::File::open(state_folder) {
      let mut reader = std::io::BufReader::new(state_file);
      let mut line = String::new();
      let _ = reader.read_line(&mut line);
      if line.len() > 0 {
        return line.chars().nth(0).unwrap() == '1';
      }
    }
  }

  let available = match std::process::Command::new(chromium_path)
    .args(["--headless", "--dump-dom", "www.google.com"])
    .output()
  {
    Ok(output) => {
      if output.status.success() {
        true
      } else {
        false
      }
    }
    Err(_) => false,
  };
  // Record the chromium state in a file
  if let Ok(mut state_folder) = get_state_folder() {
    state_folder.push("chromium_available");
    if let Ok(mut state_file) = std::fs::File::create(state_folder) {
      let _ = state_file.write_all(if available { b"1\n" } else { b"0\n" });
    }
  }
  return available;
}

// Fetches the page through chromium so that javascript can be interpreted if needed.
// Returns the resulting HTML content.
// ⚠️ This relies on undocumented chromium features
fn fetch_by_chromium(url: &str) -> Result<String> {
  if url.ends_with(".pdf") {
    anyhow::bail!("chromium is not able to download pdf");
  }
  let output = std::process::Command::new("chromium")
    .args(["--headless", "--dump-dom", url])
    .output()
    .expect("could not spawn chromium");
  let content = String::from_utf8(output.stdout)?;
  // That's how chromium tells you he's unhappy
  if content.starts_with(r#"<html><head><script>start("/");</script>"#) {
    anyhow::bail!("could not load {}", url)
  }
  Ok(content)
}

use candle_core::{Device, Tensor};

// from https://github.com/huggingface/candle/blob/26c16923b92bddda6b05ee1993af47fb6de6ebd7/candle-examples/examples/bert/main.rs
fn compute_embeddings(content: &str) -> Result<Tensor, Box<dyn Error + Send + Sync>> {
  use candle_nn::VarBuilder;
  use candle_transformers::models::bert::{BertModel, Config, DTYPE};
  use tokenizers::Tokenizer;

  let device = &Device::Cpu;
  let mut tokenizer_builder = Tokenizer::from_file("./all-MiniLM-L6-v2/tokenizer.json")?;
  let config = std::fs::read_to_string("./all-MiniLM-L6-v2/config.json")?;
  let config: Config = serde_json::from_str(&config)?;
  let vb = VarBuilder::from_pth("./all-MiniLM-L6-v2/pytorch_model.bin", DTYPE, device)?;
  let model = BertModel::load(vb, &config)?;
  // let start = std::time::Instant::now();
  let tokenizer = tokenizer_builder
    .with_padding(None)
    .with_truncation(None)
    .map_err(E::msg)?;
  let tokens = tokenizer
    .encode(content, true)
    .map_err(E::msg)?
    .get_ids()
    .to_vec();

  let token_ids = Tensor::new(tokens.as_slice(), device)?.unsqueeze(0)?;
  let token_type_ids = token_ids.zeros_like()?;
  // println!("Loaded and encoded {:?}", start.elapsed());

  // let start = std::time::Instant::now();
  let embeddings = model.forward(&token_ids, &token_type_ids, None)?;
  // This will give as an embedding per token so we apply some avg-pooling by
  // taking the mean embedding value for all tokens (including padding)
  let (_n_sentence, n_tokens, _hidden_size) = embeddings.dims3()?;
  let embeddings = (embeddings.sum(1)? / (n_tokens as f64))?;
    // from dimension [1, 384] to [384]
  let embeddings = embeddings.squeeze(0)?;
  // println!("Took {:?}", start.elapsed());

  Ok(embeddings)
}

fn similarity(e_i: Tensor, e_j: Tensor) -> Result<f32> {
  let sum_ij = (&e_i * &e_j)?.sum_all()?.to_scalar::<f32>()?;
  let sum_i2 = (&e_i * &e_i)?.sum_all()?.to_scalar::<f32>()?;
  let sum_j2 = (&e_j * &e_j)?.sum_all()?.to_scalar::<f32>()?;
  let cosine_similarity = sum_ij / (sum_i2 * sum_j2).sqrt();
  return Ok(cosine_similarity);
}

// reference: https://www.reddit.com/r/rust/comments/1hyfex8/comment/m6kce24/
fn search(config: &Config, bookmarks: &Vec<Bookmark>, needle: &Vec<String>) -> Result<(), Box<dyn Error + Send + Sync>> {
  let needle = needle.join(" ");

  let needle_embeddings = compute_embeddings(&needle)?;
  // println!(">{needle_embeddings}");

  // Retrieve all the path in the data folder that ends with .embeddings
  let embedding_paths = std::fs::read_dir(get_data_folder()?)?
    .filter(|r| r.is_ok()) // Get rid of Err variants for Result<DirEntry>
    .map(|r| r.unwrap()) // This is safe, since we only have the Ok variants
    .filter(|dir_entry| {
      if let Ok(file_type) = dir_entry.file_type() {
        file_type.is_file()
      } else { false }
    })
    .filter(|dir_entry| {
      if let Some(extension) = dir_entry.path().extension() {
        extension == "embeddings"
      } else { false }
    })
    .map(|dir_entry| dir_entry.path());

  let mut similarities = embedding_paths
    .map(|embedding_path| {
      let inputfile = std::fs::File::open(&embedding_path)?;
      let article_embeddings: Vec<f32> = serde_json::from_reader(inputfile)?;
      let length = article_embeddings.len();
      let article_embeddings = Tensor::from_vec(article_embeddings, length, &Device::Cpu)?;
      let similarity = similarity(needle_embeddings.clone(), article_embeddings)?;
      Ok::<(f32, PathBuf), E>((similarity, embedding_path))
    })
    .filter(|r| r.is_ok()) // Get rid of Err variants for Result<DirEntry>
    .map(|r| r.unwrap()) // This is safe, since we only have the Ok variants
    .collect::<Vec<_>>();
  similarities.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

  for entry in similarities.iter().take(5) {
    let embedding_path = &entry.1;
    let hash = embedding_path.file_stem().unwrap();
    if let Some(bookmark) = bookmarks.iter().find(|b| hash.to_str().unwrap().starts_with(&b.hash)) {
      println!("{} {}", entry.0, bookmark.href);
    }
  }

  Ok(())
}

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
  let default_config_file_path: String = env::var("XDG_CONFIG_HOME")
    .unwrap_or(env::var("HOME")? + "/.config/")
    + "/bookmark/config.yaml";

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
    if std::fs::metadata(std::path::Path::new(&default_config_file_path)).is_ok() {
      // Try to load it
      match std::fs::read_to_string(&default_config_file_path) {
        Err(e) => {
          // We should be able to load a config from somewhere, if we fail just
          // stops execution
          eprintln!("error: {e}: {}", default_config_file_path);
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

  // Load the bookmark files or create it if it does not exists
  let mut bookmarks: Vec<Bookmark> = {
    let inputfile = match std::fs::OpenOptions::new()
      .read(true)
      .write(true)
      .create(true)
      .open(&config.bookmarks) {
      Ok(inputfile) => inputfile,
      Err(e) => {
        eprintln!("{}: {}", config.bookmarks.display(), e);
        std::process::exit(1);
      }
    };
    match inputfile.metadata() {
      // serde does not accpet empty files
      Ok(metadata) if metadata.len() == 0 => vec![],
      Ok(metadata) => match serde_json::from_reader(std::io::BufReader::new(inputfile)) {
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
      Err(e) => {
        eprintln!("{}: {}", config.bookmarks.display(), e);
        std::process::exit(1);
      }
    }
  };
  // Everytime bookmark runs, it remove duplicates
  let new_bookmarks = dedup(&bookmarks[..], &config.bookmarks)?;
  if new_bookmarks.len() < bookmarks.len() {
    println!("deduped {} entries", bookmarks.len() - new_bookmarks.len());
    bookmarks = new_bookmarks;
  }
  // The object used to retrieve the content of bookmark
  let url_store = UrlStore::new(&config)?;
  // We treat the commands here
  match &opt.command {
    Some(Commands::Add { url }) => add(&config, &url_store, &mut bookmarks, &url)?,
    Some(Commands::Fetch { urls }) => fetch_urls(&url_store, &urls)?,
    Some(Commands::Hash { hash }) => hash2url(&config, &bookmarks, &hash)?,
    Some(Commands::Search { needle }) => search(&config, &bookmarks, &needle)?,
    None => {
      // By default, just lists the bookmarks
      for i in 0..bookmarks.len() {
        println!("{} {} ({})", i + 1, bookmarks[i].title, bookmarks[i].href);
      }
    }
  }

  Ok(())
}
