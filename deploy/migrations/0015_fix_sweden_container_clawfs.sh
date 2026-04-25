# Description: Fix ClawFS for Sweden GPU containers — remove FUSE remnants, set up mc
#
# Context: Sweden GPU containers (boris, peabody, sherman, snidely, dudley) were
# onboarded with a version of bootstrap.sh that installed JuiceFS 1.3.1 + fuse3 and
# attempted a FUSE mount at /mnt/clawfs pointing to redis://146.190.134.110:6379/1.
# This fails because:
#   (a) Container seccomp policy blocks /dev/fuse access (no --device /dev/fuse)
#   (b) Redis only listens on the Tailscale IP (100.89.199.14), not the public IP
#
# The correct architecture (see docs/clawfs.md) is to use mc (MinIO client) via the
# JuiceFS S3 gateway on the hub (port 9100), which is publicly accessible and requires
# no FUSE device, no kernel module, no container capability changes.
#
# This migration:
#   1. Removes any supervisord clawfs/juicefs FUSE entries (container-specific)
#   2. Kills any running juicefs mount processes trying to use /mnt/clawfs
#   3. Cleans up /mnt/clawfs if it exists and is not a real mount
#   4. Removes CLAWFS_REDIS_URL and CLAWFS_MOUNT from .env (if still present)
#   5. Installs mc (MinIO client) if not already present
#   6. Configures mc alias for the hub S3 gateway (port 9100)
#
# Condition: Linux nodes — primarily Sweden GPU containers

on_platform linux || { m_skip "Linux only — nothing to do on macOS"; exit 0; }

CCC_ENV="${CCC_DIR}/.env"
[[ -f "$CCC_ENV" ]] && { set -a; source "$CCC_ENV"; set +a; }

# ── 1. Remove supervisord clawfs/juicefs entries ─────────────────────────
# Containers use supervisord instead of systemd. Clean up any conf.d file that
# tries to run juicefs mount.
_removed_sup=0
for _conf_dir in /etc/supervisor/conf.d /etc/supervisord.d; do
  if [[ -d "$_conf_dir" ]]; then
    for _f in "$_conf_dir"/*.conf; do
      [[ -f "$_f" ]] || continue
      if grep -qiE 'juicefs|clawfs' "$_f" 2>/dev/null; then
        m_info "Removing supervisord config: $_f"
        sudo rm -f "$_f" && m_success "Removed $_f" || m_warn "Could not remove $_f"
        _removed_sup=$((_removed_sup + 1))
      fi
    done
  fi
done

if [[ "$_removed_sup" -gt 0 ]]; then
  # Reload supervisord to pick up the removed program
  sudo supervisorctl -c /etc/supervisord.conf reread 2>/dev/null && \
  sudo supervisorctl -c /etc/supervisord.conf update 2>/dev/null || \
  sudo supervisorctl reread 2>/dev/null && sudo supervisorctl update 2>/dev/null || \
  m_warn "supervisorctl reload failed — restart supervisord to apply changes"
else
  m_skip "No supervisord juicefs/clawfs entries found"
fi

# ── 2. Kill any running juicefs mount processes (not gateway) ────────────
# Only kill 'juicefs mount' processes. Do NOT kill 'juicefs gateway' (that's
# the hub's S3 service). Check if this node is the hub by looking for the
# gateway listening on port 9100.
if ! ss -tlnp 2>/dev/null | grep -q ':9100.*juicefs'; then
  # Not the hub — safe to kill juicefs mount processes
  if pgrep -f 'juicefs mount' >/dev/null 2>&1; then
    m_info "Killing juicefs mount processes..."
    pkill -f 'juicefs mount' 2>/dev/null || true
    m_success "juicefs mount processes killed"
  else
    m_skip "No juicefs mount processes running"
  fi
else
  m_skip "Hub node (juicefs gateway running on :9100) — not killing juicefs processes"
fi

# ── 3. Clean up /mnt/clawfs if it exists ─────────────────────────────────
# Skip on the hub — the hub intentionally keeps /mnt/clawfs mounted via FUSE.
CONTAINER_MOUNT="/mnt/clawfs"
if ss -tlnp 2>/dev/null | grep -q ':9100.*juicefs'; then
  m_skip "Hub node — leaving $CONTAINER_MOUNT mounted (hub's own FUSE mount)"
elif mountpoint -q "$CONTAINER_MOUNT" 2>/dev/null; then
  m_info "Unmounting $CONTAINER_MOUNT..."
  if fusermount3 -u "$CONTAINER_MOUNT" 2>/dev/null || \
     fusermount -u "$CONTAINER_MOUNT" 2>/dev/null || \
     umount "$CONTAINER_MOUNT" 2>/dev/null; then
    m_success "$CONTAINER_MOUNT unmounted"
  else
    m_warn "Could not unmount $CONTAINER_MOUNT — may need manual cleanup or reboot"
  fi
elif [[ -d "$CONTAINER_MOUNT" ]]; then
  m_skip "$CONTAINER_MOUNT directory exists but is not mounted — leaving in place"
else
  m_skip "$CONTAINER_MOUNT does not exist"
fi

# ── 4. Remove stale CLAWFS_* and FUSE vars from .env ─────────────────────
if [[ -f "$CCC_ENV" ]]; then
  _removed_vars=0
  for _key in CLAWFS_ENABLED CLAWFS_MOUNT CLAWFS_REDIS_URL CLAWFS_CACHE_DIR \
              CLAWFS_CCC_REPO CCC_REPO_PUSHER; do
    if grep -q "^${_key}=" "$CCC_ENV" 2>/dev/null; then
      sed -i "/^${_key}=/d" "$CCC_ENV" && _removed_vars=$((_removed_vars + 1))
    fi
  done
  if [[ "$_removed_vars" -gt 0 ]]; then
    m_success "Removed $_removed_vars stale CLAWFS_* vars from .env"
  else
    m_skip "No stale CLAWFS_* vars in .env"
  fi
else
  m_skip ".env not found at $CCC_ENV"
fi

# ── 5. Install mc (MinIO client) if not present ───────────────────────────
if command -v mc &>/dev/null; then
  m_skip "mc already installed: $(mc --version 2>/dev/null | head -1)"
else
  m_info "Installing mc (MinIO client)..."
  MC_BIN="/usr/local/bin/mc"
  _arch="$(uname -m)"
  case "$_arch" in
    x86_64)  _mc_arch="amd64" ;;
    aarch64) _mc_arch="arm64" ;;
    *)       m_warn "Unknown arch $_arch — skipping mc install"; _mc_arch="" ;;
  esac
  if [[ -n "$_mc_arch" ]]; then
    MC_URL="https://dl.min.io/client/mc/release/linux-${_mc_arch}/mc"
    if command -v curl &>/dev/null; then
      if sudo curl -sSfL "$MC_URL" -o "$MC_BIN" && sudo chmod +x "$MC_BIN"; then
        m_success "mc installed at $MC_BIN"
      else
        m_warn "mc download failed — ClawFS S3 access will not work until mc is installed"
        m_warn "  Install manually: curl -sSfL $MC_URL -o ~/bin/mc && chmod +x ~/bin/mc"
      fi
    else
      m_warn "curl not found — cannot install mc"
    fi
  fi
fi

# ── 6. Configure mc alias for ClawFS S3 gateway ──────────────────────────
# The hub exposes the JuiceFS S3 gateway on port 9100 (publicly accessible).
# Credentials are fetched from the CCC secrets API.
if command -v mc &>/dev/null; then
  MINIO_ALIAS="${MINIO_ALIAS:-ccc-hub}"
  CCC_URL="${CCC_URL:-}"
  # Support both old (RCC_AGENT_TOKEN) and new (CCC_AGENT_TOKEN) variable names
  CCC_AGENT_TOKEN="${CCC_AGENT_TOKEN:-${RCC_AGENT_TOKEN:-}}"

  if [[ -z "$CCC_URL" ]]; then
    m_warn "CCC_URL not set — cannot auto-configure mc alias"
    m_warn "  Set manually: mc alias set ccc-hub http://<hub-ip>:9100 <ak> <sk>"
  else
    # Derive the hub S3 gateway URL from CCC_URL (replace port with 9100)
    _hub_base="${CCC_URL%:*}"  # strip port
    _clawfs_url="${_hub_base}:9100"

    # Check if alias is already configured and working
    if mc ls "$MINIO_ALIAS" >/dev/null 2>&1; then
      m_skip "mc alias '$MINIO_ALIAS' already configured and working"
    else
      m_info "Configuring mc alias '$MINIO_ALIAS' → $_clawfs_url ..."
      # Fetch credentials from CCC hub secrets API
      _ak=""
      _sk=""
      if [[ -n "$CCC_AGENT_TOKEN" ]]; then
        _ak=$(curl -sf -H "Authorization: Bearer ${CCC_AGENT_TOKEN}" \
          "${CCC_URL}/api/secrets/agentfs%2Faccess_key" 2>/dev/null | \
          python3 -c "import json,sys; print(json.load(sys.stdin).get('value',''))" 2>/dev/null || true)
        _sk=$(curl -sf -H "Authorization: Bearer ${CCC_AGENT_TOKEN}" \
          "${CCC_URL}/api/secrets/agentfs%2Fsecret_key" 2>/dev/null | \
          python3 -c "import json,sys; print(json.load(sys.stdin).get('value',''))" 2>/dev/null || true)
      fi

      if [[ -n "$_ak" && -n "$_sk" ]]; then
        mc alias set "$MINIO_ALIAS" "$_clawfs_url" "$_ak" "$_sk" >/dev/null 2>&1 && \
          m_success "mc alias '$MINIO_ALIAS' configured → $_clawfs_url" || \
          m_warn "mc alias set failed — check credentials"
        # Persist to .env
        if [[ -f "$CCC_ENV" ]]; then
          sed -i "/^MINIO_ALIAS=/d; /^CLAWFS_URL=/d" "$CCC_ENV" 2>/dev/null || true
          printf 'MINIO_ALIAS=%s\nCLAWFS_URL=%s\n' "$MINIO_ALIAS" "$_clawfs_url" >> "$CCC_ENV"
          m_success "MINIO_ALIAS and CLAWFS_URL written to .env"
        fi
      else
        m_warn "Could not fetch agentfs credentials from hub — mc alias not configured"
        m_warn "  Configure manually after obtaining credentials:"
        m_warn "  mc alias set $MINIO_ALIAS $_clawfs_url <access_key> <secret_key>"
        m_warn "  Then verify: mc ls $MINIO_ALIAS/clawfs/"
      fi
    fi
  fi
else
  m_skip "mc not installed — skipping alias configuration"
fi

m_success "Migration 0015 complete — FUSE setup removed, mc configured for S3 gateway"
m_info "ClawFS access: mc ls ${MINIO_ALIAS:-ccc-hub}/clawfs/"
m_info "See docs/clawfs.md for full usage guide"
