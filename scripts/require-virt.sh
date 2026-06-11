#!/bin/sh
# Pre-push gate: refuse to push from a host without hardware virtualization.
#
# `make test` skips the VM-boot integration tests when VMs cannot boot here
# (same probe as tests/integration/utils.rs `has_virtualization`) — e.g.
# inside a sodagun sandbox guest. A green test run on such a host therefore
# does not cover them; run the full suite on a virtualization-capable host
# and push from there.

case "$(uname -s)" in
Linux) [ -e /dev/kvm ] && exit 0 ;;
Darwin) [ "$(sysctl -n kern.hv_support 2>/dev/null)" = "1" ] && exit 0 ;;
esac

echo "push blocked: no hardware virtualization here, so the VM-boot integration tests were skipped." >&2
echo "Run 'make test' on a virtualization-capable host (e.g. the sandbox host) and push from there." >&2
exit 1
