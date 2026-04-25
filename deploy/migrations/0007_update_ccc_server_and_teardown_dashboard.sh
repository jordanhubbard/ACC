# Description: Update ccc-server service (DASHBOARD_DIST + EnvironmentFile) and remove ccc-dashboard
#
# Context: Two changes on hub nodes:
#   1. ccc-server.service gains DASHBOARD_DIST (ClawChat WASM SPA path) and EnvironmentFile
#      so it serves the UI directly and picks up QDRANT_FLEET_KEY etc. from ~/.ccc/.env
#   2. ccc-dashboard.service is superseded by ccc-server serving the UI itself.
#      dashboard-server binary was removed in the dashboard-ui/dashboard-server cleanup.
# Condition: linux (systemd), hub nodes only

if [ "${IS_HUB:-false}" = "true" ] || systemctl is-active --quiet ccc-server 2>/dev/null; then
    on_platform linux

    # Re-install ccc-server.service with updated config
    systemd_install deploy/systemd/ccc-server.service ccc-server.service

    # Tear down ccc-dashboard.service (dashboard-server binary deleted)
    systemd_teardown ccc-dashboard.service \
      /etc/systemd/system/ccc-dashboard.service \
      /usr/lib/systemd/system/ccc-dashboard.service
fi
