# ECR credential helper (remove the unencrypted-Docker-credentials warning)

## Symptom

Running `scripts/aws-redeploy.sh` or `scripts/aws-test-deploy.sh` prints:

```
WARNING! Your credentials are stored unencrypted in '/home/<you>/.docker/config.json'.
Configure a credential helper to remove this warning.
```

## Why

Both deploy scripts authenticate to ECR before `docker push`. The classic
form `aws ecr get-login-password | docker login` makes Docker persist the
returned ECR auth token — a bearer token valid ~12h, scoped to this
account's ECR — in cleartext in `~/.docker/config.json`. For that window
any process running as your user can read it, and it leaks into `~`
backups / dotfile sync.

## Fix (one-time, recommended): amazon-ecr-credential-helper

Install `docker-credential-ecr-login`, then map the registry to it. Docker
will invoke the helper on demand at `docker push` time — it fetches a fresh
token from your AWS CLI credential chain each time and **never writes one
to disk**. The deploy scripts (#55) detect this configuration and skip the
explicit `docker login` entirely.

### 1. Install the helper

- **Fedora / RHEL:** `sudo dnf install amazon-ecr-credential-helper`
- **Debian / Ubuntu:** `sudo apt install amazon-ecr-credential-helper`
- **macOS:** `brew install docker-credential-helper-ecr`
- **From source:** `go install github.com/awslabs/amazon-ecr-credential-helper/ecr-login/cli/docker-credential-ecr-login@latest`
  (then ensure `$(go env GOPATH)/bin` is on `PATH`)

Verify: `command -v docker-credential-ecr-login` prints a path.

### 2. Map the ECR registry to the helper

Add a `credHelpers` entry to `~/.docker/config.json` for your account's
registry (replace `<account>` / `<region>`):

```json
{
  "credHelpers": {
    "<account>.dkr.ecr.<region>.amazonaws.com": "ecr-login"
  }
}
```

If the file already exists, merge the `credHelpers` key in rather than
overwriting it. Quick merge with `jq` (`<account>`/`<region>` substituted):

```sh
REGISTRY="<account>.dkr.ecr.<region>.amazonaws.com"
cfg=~/.docker/config.json
[ -f "$cfg" ] || echo '{}' > "$cfg"
tmp=$(mktemp)
jq --arg r "$REGISTRY" '.credHelpers[$r] = "ecr-login"' "$cfg" > "$tmp" && mv "$tmp" "$cfg"
```

### 3. (optional) Drop the stale token already on disk

A previously-written token sits under `auths` in the same file — remove it
and re-run a deploy; the warning is gone and no new token is written:

```sh
jq 'del(.auths)' ~/.docker/config.json > /tmp/dc.json && mv /tmp/dc.json ~/.docker/config.json
```

## Alternatives

- **`docker-credential-secretservice`** — stores tokens in GNOME Keyring /
  KWallet via the freedesktop SecretService API. Set
  `{ "credsStore": "secretservice" }`. Idiomatic on Linux desktops, but
  still materializes a token (just encrypted at rest) and isn't ECR-aware.
- **`docker-credential-pass`** — GPG-backed `pass` store; lighter, needs
  `gpg` set up.

The ECR helper is preferred here because every login the scripts perform is
to ECR, and it removes the on-disk token entirely rather than encrypting it.

## Without the helper

The deploy scripts still work unchanged — they fall back to
`aws ecr get-login-password | docker login` and you'll keep seeing the
warning. The token is short-lived and account-scoped, so this is low risk;
the helper is hygiene, not a hard requirement.
