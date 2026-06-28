#!/usr/bin/env bash
# Optional, deterministic DNS override for Docker Chrome containers.
#
# Some Docker/host-network combinations inherit a stub resolver (for example
# 127.0.0.53) that is not reachable from Chrome in the container. Set
# VOIDCRAWL_DNS_SERVERS="1.1.1.1,8.8.8.8" (or whitespace-separated) to rewrite
# resolv.conf at container start instead of patching it by hand.
set -euo pipefail

servers_raw="${VOIDCRAWL_DNS_SERVERS:-}"
if [[ -z "${servers_raw//[[:space:],]/}" ]]; then
    exit 0
fi

resolv_conf="${VOIDCRAWL_RESOLV_CONF_PATH:-/etc/resolv.conf}"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

validate_line_value() {
    local name="$1"
    local value="$2"
    local pattern="$3"
    if [[ ! "$value" =~ $pattern ]]; then
        echo "[dns] invalid $name value" >&2
        exit 2
    fi
}

valid_count=0
# Split on commas and whitespace.
for server in ${servers_raw//,/ }; do
    [[ -n "$server" ]] || continue
    if [[ ! "$server" =~ ^[0-9A-Fa-f:.]+$ ]]; then
        echo "[dns] invalid nameserver '$server' in VOIDCRAWL_DNS_SERVERS" >&2
        exit 2
    fi
    printf 'nameserver %s\n' "$server" >> "$tmp"
    valid_count=$((valid_count + 1))
done

if [[ "$valid_count" -eq 0 ]]; then
    echo "[dns] VOIDCRAWL_DNS_SERVERS was set but contained no nameservers" >&2
    exit 2
fi

if [[ -n "${VOIDCRAWL_DNS_SEARCH:-}" ]]; then
    validate_line_value "VOIDCRAWL_DNS_SEARCH" "$VOIDCRAWL_DNS_SEARCH" '^[A-Za-z0-9._ -]+$'
    printf 'search %s\n' "$VOIDCRAWL_DNS_SEARCH" >> "$tmp"
fi
options="${VOIDCRAWL_DNS_OPTIONS:-timeout:2 attempts:3 rotate}"
validate_line_value "VOIDCRAWL_DNS_OPTIONS" "$options" '^[A-Za-z0-9:._ -]+$'
printf 'options %s\n' "$options" >> "$tmp"

if cp "$tmp" "$resolv_conf"; then
    echo "[dns] wrote $valid_count nameserver(s) to $resolv_conf"
else
    echo "[dns] failed to write $resolv_conf; set Docker --dns or run container writable" >&2
    exit 2
fi
