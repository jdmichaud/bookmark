# bookmark

```
bookmark [-hv] [-c/--config path] [-b/--bookmarks path] add url
bookmark [-hv] [-c/--config path] [-b/--bookmarks path] search query
bookmark --print-config
```

# Description

`bookmark` is a command line tool to manage store and bookmark. By default
`bookmark` will backup urls into a json file.

Depending on the configuration, `bookmark` can fetch a copy of the page pointed
to by the url into a local file, index the content of that file and use semantic
search to retrieve the best matching bookmark corresponding to a query.

On startup, `bookmark` will eliminate duplicates in the bookmark file if any are
found.

Depending on the configuration, `bookmark` can download the content pointed to
by the url once interpreted by `chrome` into local files as a way to backup the
bookmark content.

## Commands

### add

Add a bookmark.

```
bookmark add <URL>
```

### search

Search a bookmark using vector embeddings of the bookmark content.

```
bookmark search a natural query that can contain several words
```

### check

Check that urls are fetched and indexed if configured so.

```
bookmark check
```

## Config

`bookmark` use a config from either:
- $XDG_CONFIG_HOME/bookmark/config.yaml or
- $HOME/.config/bookmark/config.yaml
in that order.

You can also use the `-c/--config` option.

`bookmark` can print a default config help comments:
```
bookmark --print-config
```
Redirect that to a config file and you can then modify the config.

## Files

The bookmark is created/modified either
- in the current working directory or
- in the path provided by `-c/--bookmarks` or
- in the path provided in the config file.

In addition to the aforementioned config file, `bookmark`, if configured so,
will save the url content as interpreted by chrome (necessitate chrome) in
either:
- $XDG_DATA_HOME/bookmark/
- $HOME/.local/share/bookmark/

`bookmark` save an opaque state in either:
- $XDG_STATE_HOME/bookmark/
- $HOME/.local/state/bookmark/

## Build

You need to musl development tools (for a static build) and protbuf compiler
(for candle-onnx):
```bash
apt install musl-dev musl-tools
```
