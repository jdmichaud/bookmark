```
A bookmark manager

Usage: bookmark [OPTIONS] [COMMAND]

Commands:
  add   Adds a bookmark
  help  Print this message or the help of the given subcommand(s)

Options:
  -c, --config <FILE>     YAML configuration file to use.
                          If not provided, use the file ~/.config/bookmark/config.yaml.
                          If the file does not exists, use default embedded config (see
                          --print-config)
      --print-config      Print the used configuration and exit.
                          You can use this option to initialize the default config file with:
                            mkdir -p ~/.config/bookmark/
                            bookmark --print-config ~/.config/bookmark/config.yaml
  -b, --bookmarks <FILE>  Override the configured bookmark file
  -h, --help              Print help
  -V, --version           Print version
```

#### add

```
Adds a bookmark

Usage: bookmark add <URL>

Arguments:
  <URL>  

Options:
  -h, --help  Print help
```
