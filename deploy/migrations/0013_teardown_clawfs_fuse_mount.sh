# Description: Remove ClawFS FUSE mount — agents now use the S3 gateway instead
#
# Context: The JuiceFS FUSE mount (~/clawfs) has been removed from the CCC agent
# setup. Agent memory sync now uses the JuiceFS S3 gateway on the hub (port 9100)
# via clawfs-sync, which requires no local FUSE mount, no JuiceFS binary, and no
# macFUSE on macOS. The clawfs-${AGENT}.service systemd unit (dynamically created
# by bootstrap.sh) and clawfs-sparky.service are no longer deployed.
# Condition: Linux nodes that ran bootstrap.sh before this migration.

on_platform linux

# ── 1. Tear down any running clawfs-*.service units ───────────────────────
# The unit was named clawfs-${AGENT_NAME}.service — enumerate all matches.
for svc in $(systemctl list-units --no-legend 'clawfs-*.service' 2>/dev/null | awk '{print $1}'); do
  svc_path=$(systemctl show "$svc" -p FragmentPath --value 2>/dev/null || true)
  systemd_teardown "$svc" "$svc_path" \
    "/etc/systemd/system/${svc}" \
    "/usr/lib/systemd/system/${svc}"
  m_success "Removed $svc"
done

# Also remove clawfs-sparky.service if it was deployed from the template
systemd_teardown clawfs-sparky.service \
  /etc/systemd/system/clawfs-sparky.service \
  /usr/lib/systemd/system/clawfs-sparky.service

# ── 2. Unmount ~/clawfs if still mounted ─────────────────────────────────
CLAWFS_MOUNT="${CLAWFS_MOUNT:-$HOME/clawfs}"
if mountpoint -q "$CLAWFS_MOUNT" 2>/dev/null; then
  if command -v juicefs &>/dev/null; then
    juicefs umount "$CLAWFS_MOUNT" 2>/dev/null && m_success "Unmounted $CLAWFS_MOUNT" || \
      m_warn "juicefs umount failed — try: fusermount -u $CLAWFS_MOUNT"
  else
    fusermount -u "$CLAWFS_MOUNT" 2>/dev/null || umount "$CLAWFS_MOUNT" 2>/dev/null || \
      m_warn "Could not unmount $CLAWFS_MOUNT — reboot will clear it"
  fi
else
  m_skip "$CLAWFS_MOUNT is not mounted"
fi

# ── 3. Remove stale CLAWFS_* vars from ~/.ccc/.env ───────────────────────
ENV_FILE="${CCC_DIR}/.env"
if [ -f "$ENV_FILE" ]; then
  for key in CLAWFS_ENABLED CLAWFS_MOUNT CLAWFS_REDIS_URL CLAWFS_CACHE_DIR CLAWFS_CCC_REPO CCC_REPO_PUSHER; do
    sed -i "/^${key}=/d" "$ENV_FILE" 2>/dev/null || true
  done
  m_success "Removed stale CLAWFS_* vars from $ENV_FILE"
fi
