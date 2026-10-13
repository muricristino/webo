<img width="1380" height="865" alt="image" src="https://github.com/user-attachments/assets/512d3f4d-7c97-42c4-8212-5cd8a44b2ba9" />

# webo

A health panel for your server — CPU, memory, disk, temperature, battery and
network on a single screen, genuinely lightweight (one Rust binary, < 50 MB of
RAM), with a JSON API ready for automation and agents (MCP).

```
docker compose -f deploy/docker-compose.yml up -d --build
# panel at http://your-server:5050
```

## What it shows

- **CPU** — usage, load, 24 h sparkline
- **Memory** — used/total, sparkline
- **Disk** — used/total/free of the root filesystem
- **Temperature** — CPU, with a visual alert at 85 °C
- **Battery** — charge, status and charge limit (laptop servers)
- **Network** — download/upload rates
- **System** — OS, kernel, architecture, process count

Cards for metrics your hardware doesn't have (a battery in a datacenter, say)
simply disappear.

## API (the same contract the panel uses)

| Endpoint | Returns |
|---|---|
| `GET /api/v1/snapshot` | everything about the current moment |
| `GET /api/v1/history?minutes=1440` | CPU/RAM/network series (15 s samples, 24 h in memory) |
| `GET /api/v1/system` | machine identity (hostname, OS, kernel, hardware) |
| `GET /healthz` | `ok` |

Nothing is UI-only: everything the panel shows comes from these endpoints —
they are how an MCP server (or any automation) sees the machine.

The full reference — authentication, the 19 MCP tools, and what an agent is
allowed to change — is in [`docs/`](docs/).

## Configuration

| Env | Default | What |
|---|---|---|
| `WEBO_BIND` | `0.0.0.0:5050` | listen address |
| `WEBO_SAMPLE_SECS` | `15` | sampling interval |
| `WEBO_NET_DEV` | `/proc/net/dev` | network rates source (in a container, mount the host's) |
| `WEBO_HOSTNAME` | — | overrides the reported hostname |
| `WEBO_PUBLIC_URL` | the request's host | public address used in the agent install guide, the skill and the browser error snippet |
| `WEBO_NAME` | `webo` | name the MCP server and skill get in an agent; set one per server when someone uses several |
| `CLERK_PUBLISHABLE_KEY` · `CLERK_SECRET_KEY` | — | turn on login; without them webo runs open (see [authentication](docs/authentication.md)) |
| `WEBO_ALLOWED_EMAILS` | — | who may sign in, comma-separated |
| `WEBO_RUNBOOK` | built-in | markdown file describing this server, served to agents as `webo://runbook` |
| `WEBO_DEPLOY_ACTION` | `muricristino/webo/deploy-action@main` | GitHub Action the deploy workflow of each app uses |
| `WEBO_DEPLOY_HOST` | — | SSH target on the tailnet (`user@address`) that apps deploy to; written to each app repo as a secret |

The compose mounts (`/sys`, `/proc/net/dev`, `/etc/hostname`, `/etc/os-release`)
are optional — without them webo still works and just hides the corresponding
metrics.

## Security

Without Clerk keys webo runs in local mode, with everything open: fine on a
laptop, never on a server reachable from the internet. Docker publishes ports
around the host firewall (ufw does not see them), so `5050:5050` on a public
machine is a public panel.

To expose webo, set the Clerk keys and publish it through a Cloudflare Tunnel,
with no port open on the host:

```
docker compose -f deploy/docker-compose.yml -f deploy/docker-compose.tunnel.yml up -d --build
```

## License

[MIT](LICENSE)

Built by [muricristino](https://github.com/muricristino).
