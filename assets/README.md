# Character assets

`assets/agents/` holds Microsoft Agent character files (`.acs` / `.acf` / `.acd`) used by
the examples and the real-file integration tests. **These files are third-party and are
git-ignored** — we don't have rights to redistribute Microsoft's (or other authors')
character artwork.

To run the examples/tests against real characters, drop your own files into
`assets/agents/` locally, e.g.:

```
assets/agents/Merlin.acs
assets/agents/CLIPPIT.ACS
assets/agents/GENIUS.ACS
```

Then:

```sh
cargo run -p da-format --example dump   -- assets/agents/Merlin.acs
cargo run -p da-format --example render -- assets/agents/Merlin.acs Greet
cargo test   # real_files test picks up whatever is present, and skips if none
```

Classic MS Agent characters were distributed by Microsoft and various character authors;
source them yourself. This directory (minus this README) stays out of version control.
