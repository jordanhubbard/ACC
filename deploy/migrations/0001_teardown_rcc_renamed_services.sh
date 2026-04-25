# Description: Remove rcc-agent and rcc-dashboard services renamed in the rcc→ccc rebrand
#
# Context: In commit 03be35e the service files were renamed rcc-agent → ccc-agent and
# rcc-dashboard → ccc-dashboard. The old units may still be installed on agents that
# were set up before the rename. This migration stops, disables, and removes them.

systemd_teardown rcc-agent.service \
  /etc/systemd/system/rcc-agent.service \
  /usr/lib/systemd/system/rcc-agent.service

systemd_teardown rcc-agent.timer \
  /etc/systemd/system/rcc-agent.timer \
  /usr/lib/systemd/system/rcc-agent.timer

systemd_teardown rcc-dashboard.service \
  /etc/systemd/system/rcc-dashboard.service \
  /usr/lib/systemd/system/rcc-dashboard.service

launchd_teardown com.rcc.agent \
  ~/Library/LaunchAgents/com.rcc.agent.plist

launchd_teardown com.rcc.claude-main \
  ~/Library/LaunchAgents/com.rcc.claude-main.plist
