# Description: Install ccc-agent pull service/timer (systemd) or LaunchAgent (macOS)
#
# Context: The ccc-agent service was broken because it used %i/%h specifiers only valid
# for template units (commit 912ed9e fixes this). This migration re-installs the unit
# with AGENT_USER/AGENT_HOME substituted at install time.

if on_platform linux; then
  systemd_install deploy/systemd/ccc-agent.service ccc-agent.service
  systemd_install deploy/systemd/ccc-agent.timer   ccc-agent.timer
fi

if on_platform macos; then
  launchd_install \
    deploy/launchd/com.ccc.agent.plist \
    ~/Library/LaunchAgents/com.ccc.agent.plist \
    com.ccc.agent
fi
