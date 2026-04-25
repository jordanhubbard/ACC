# Description: Install Linux node services (agent-listener, clawfs-metrics-push)
#
# Context: agent-listener replaced clawbus-natasha for ClawBus dispatch.
# clawfs-metrics-push replaced agentfs-metrics-push after the AgentFS→ClawFS rename.
# Both run on all Linux agent nodes.

if on_platform linux; then
  systemd_install deploy/systemd/agent-listener.service      agent-listener.service
  systemd_install deploy/systemd/clawfs-metrics-push.service clawfs-metrics-push.service
fi
