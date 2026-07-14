# DMND Stratum V2 Client — Getting Started Guide

## 1. Introduction

This guide walks you through setting up the DMND Stratum V2 Client and connecting it to the DMND pool. When you're done, you will have a fully functional Stratum V2 mining setup with **Job Declaration** — meaning *you* build your own block templates from your own Bitcoin node, instead of the pool deciding which transactions you mine.

**Your ASICs do not need to support Stratum V2.** The DMND Client accepts standard Stratum V1 connections from your miners and handles the SV2 protocol to the pool. Any stock-firmware miner works.

### How the pieces fit together

```
┌─────────┐  SV1 (stratum+tcp)  ┌──────────────┐   SV2 + Job Declaration   ┌───────────┐
│  ASICs  │ ──────────────────► │ DMND Client  │ ────────────────────────► │ DMND Pool │
└─────────┘       :32767        └──────┬───────┘          :20000           └───────────┘
                                       │ templates
                                ┌──────┴───────┐    IPC     ┌──────────────┐
                                │    sv2-tp    │ ◄────────► │ Bitcoin Core │
                                │ (Template    │  (unix     │  (your node) │
                                │  Provider)   │   socket)  └──────────────┘
                                └──────────────┘
                                     :8336
```

### Default ports

| Component | Port | Direction | Notes |
|---|---|---|---|
| sv2-tp (Template Provider) | 8336 | local only | DMND Client connects to it |
| DMND Client (stratum) | 32767 | LAN | Point your miners here |
| DMND Client (tx API) | 3001 | local only | Optional — see Section 7. **Do not expose publicly** |
| DMND Pool | 20000 | outbound | Client → pool |

Your firewall only needs to allow **outbound** connections to the pool, and LAN access from your miners to port 32767. Nothing here requires inbound internet access.

### System requirements

- Release binaries are published for multiple platforms and architectures — grab the one matching your system from the releases page, or build from source (see 4.1)
- A machine capable of running a Bitcoin node. A **pruned node works fine** (a few GB of disk); a full archival node needs ~800 GB. 4 GB+ RAM recommended
- Bitcoin Core **v30 or later** with multiprocess/IPC support

## 2. What You Need Before Starting

To mine with the DMND pool you first need a **DMND token**. Complete the registration form at https://onboarding.dmnd.work and wait for our confirmation email — it contains your token (an alphanumeric string you'll use in Section 4). If you don't see the email, check your spam folder before contacting support.

## 3. Enable Job Declaration Support

Job Declaration is the key Stratum V2 feature that lets miners build their own block templates, improving decentralization, censorship resistance, and latency. To use it, you run two components on your own infrastructure:

1. **Bitcoin Core** (v30+) with IPC enabled — your own node.
2. **Stratum V2 Template Provider (`sv2-tp`)** — a separate binary that connects to Bitcoin Core via IPC and serves block templates to the DMND Client.

Follow the setup instructions in the sv2-tp README — it covers both running Bitcoin Core with IPC enabled and running the Template Provider, and is always up to date:

**https://github.com/stratum-mining/sv2-tp#readme**

The Template Provider listens on port **8336** by default — you'll need that in the next section.

**✅ Verify:** the sv2-tp log should show a successful IPC connection to Bitcoin Core and new templates being generated as blocks arrive.

## 4. Run the DMND Client

### 4.1 Download the DMND Stratum V2 Client

Download the release binary for your platform from:

https://github.com/dmnd-pool/dmnd-client/releases

Verify checksums/signatures where provided — standard practice for any software that touches mining revenue.

Make the binary executable and run it:

```
chmod +x dmnd-client
TOKEN=<DMND-token> ./dmnd-client -l info -d <hashrate> --tp-address="127.0.0.1:<port>"
```

Where:

- `<DMND-token>` — the token you received by email during registration (Section 2).
- `<port>` — the Template Provider port (default **8336**).
- `<hashrate>` — the hashrate of the **least powerful machine** that will connect to this client. As a rule of thumb: use `250T` if your miners connect directly, or `20P` if you connect aggregator proxies. This only seeds the starting difficulty; the dynamic difficulty adjustment algorithm handles the rest.

Example (miners connecting directly):

```
TOKEN=abc123 ./dmnd-client -l info -d 250T --tp-address="127.0.0.1:8336"
```

**✅ Verify:** the client log should show a successful connection to the Template Provider and to the DMND pool, and templates being declared.

> **Building from source instead?** Install the Rust toolchain via [rustup](https://rustup.rs), clone the repository, and build a production binary with:
>
> ```
> cargo build --release
> ```
>
> The binary is produced at `./target/release/dmnd-client` — use it exactly as in the commands above.

### Configuration precedence

Every setting in this guide can be provided three ways, with the following precedence (highest wins):

1. CLI flags (e.g. `--api-base-url`)
2. `config.toml`
3. Environment variables (e.g. `API_BASE_URL`)

### 4.2 Endpoint configuration (optional)

By default, the client discovers pool addresses from the dashboard API and sends worker telemetry to the same API. You only need this section if you run behind a private gateway, proxy, or custom deployment.

- **Dashboard API base URL:** `--api-base-url` / `api_base_url` / `API_BASE_URL` / `DMND_CLIENT_API_BASE_URL`. Provide the base URL only; the client appends `/api/pool/urls` (pool discovery) and `/api/worker/entry` (telemetry).
- **Direct pool addresses:** `--pool-address` (repeatable) / `pool_addresses` / `POOL_ADDRESSES` / `POOL_ADDRESS` / `DMND_CLIENT_POOL_ADDRESSES`. When set, the client skips dashboard pool discovery and connects directly.

Example `config.toml`:

```toml
api_base_url = "https://api.example.com"
pool_addresses = ["pool-a.example.com:20000", "pool-b.example.com:20000"]
```

Example environment variables:

```
TOKEN=<DMND-token> \
API_BASE_URL=https://api.example.com \
POOL_ADDRESSES=pool-a.example.com:20000,pool-b.example.com:20000 \
./dmnd-client -l info -d 250T --tp-address="127.0.0.1:8336"
```
