# VTX

## Project structure

```
vtx/
├── crates/
│   ├── vtx-core/   # Core modules
│   ├── vtx-ui/     # Reusable UI widgets
│   └── vtx-app/    # Desktop application binary
└── Cargo.toml      # Workspace root: shared dependencies and lints
```

| Crate      | Kind | Description                   |
| ---------- | ---- | ------------------------------|
| `vtx-core` | lib  | Core modules                  |
| `vtx-ui`   | lib  | Reusable UI widgets           |
| `vtx-app`  | bin  | Main desktop application      |

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
