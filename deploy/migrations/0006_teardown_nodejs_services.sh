# Description: Stop agent-listener and clawfs-metrics-push (Node.js services removed)
#
# Context: All Node.js code deleted in Node.js→Rust migration. agent-listener.mjs
# (ClawBus exec daemon) and clawfs-metrics-push.mjs (Prometheus→ClawBus pusher)
# ran as systemd services. Their scripts no longer exist; services must be stopped,
# disabled, and removed. ccc-api-watchdog.mjs and stale-assignee-nudge.mjs crons
# are also removed.
# Condition: linux (systemd), all nodes (cron cleanup)

systemd_teardown agent-listener.service \
  /etc/systemd/system/agent-listener.service \
  /usr/lib/systemd/system/agent-listener.service

systemd_teardown clawfs-metrics-push.service \
  /etc/systemd/system/clawfs-metrics-push.service \
  /usr/lib/systemd/system/clawfs-metrics-push.service

# Remove Node.js-based cron entries
cron_remove 'ccc-api-watchdog\.mjs' 'stale-assignee-nudge\.mjs'
