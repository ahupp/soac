# crates/soac_inspector/src/main.rs

## File Responsibilities

HTTP server entrypoint for the inspector web app. It binds a local address, prints the URL, and serves the router from `soac_inspector`.

## Datatypes

- No datatypes are defined.

## Functions

- `main`: async Tokio entrypoint that builds the inspector app, binds `127.0.0.1:3000`, and serves requests through Axum.

## Context Read

- `crates/soac_inspector/src/lib.rs`

