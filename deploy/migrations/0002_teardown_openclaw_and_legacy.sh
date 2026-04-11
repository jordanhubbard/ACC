# Description: Remove legacy openclaw, crush-server, squirrelchat, mm-bridge, and agentfs services
#
# Context: Several services were removed or superseded during the openclaw→CCC migration.
# crush-server was deleted in commit be63945, squirrelchat-natasha in commit 2b2d6bb,
# mm-bridge (Mattermost) was purged in commit dec4b83, agentfs-metrics-push was superseded
# by clawfs-metrics-push, clawbus-natasha was superseded by agent-listener, and
# openclaw-register was removed when CCC onboarding replaced it.

# Legacy openclaw gateway processes — SIGTERM only, do NOT delete ~/.openclaw data
pkill -TERM -f 'openclaw.*gateway' 2>/dev/null || true
pkill -TERM -f 'claw-watchdog' 2>/dev/null || true
m_info "openclaw gateway processes terminated (data preserved)"

# Remove stale crons
cron_remove '\.openclaw/workspace' 'openclaw.*gateway' 'claw-watchdog\.sh'
cron_remove '\.rcc/workspace/deploy/agent-pull\.sh'

# Remove systemd units
systemd_teardown openclaw-register.service \
  /etc/systemd/system/openclaw-register.service \
  /usr/lib/systemd/system/openclaw-register.service

systemd_teardown crush-server.service \
  /etc/systemd/system/crush-server.service \
  /usr/lib/systemd/system/crush-server.service

systemd_teardown squirrelchat-natasha.service \
  /etc/systemd/system/squirrelchat-natasha.service \
  /usr/lib/systemd/system/squirrelchat-natasha.service

systemd_teardown mm-bridge.service \
  /etc/systemd/system/mm-bridge.service \
  /usr/lib/systemd/system/mm-bridge.service

systemd_teardown agentfs-metrics-push.service \
  /etc/systemd/system/agentfs-metrics-push.service \
  /usr/lib/systemd/system/agentfs-metrics-push.service

systemd_teardown clawbus-natasha.service \
  /etc/systemd/system/clawbus-natasha.service \
  /usr/lib/systemd/system/clawbus-natasha.service
