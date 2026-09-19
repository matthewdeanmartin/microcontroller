#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p certs
if [[ -e certs/server.key || -e certs/server.crt ]]; then
    echo 'Certificate files already exist; refusing to overwrite.' >&2
    exit 1
fi
umask 077
MSYS_NO_PATHCONV=1 openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
  -keyout certs/server.key -out certs/server.crt -days 365 \
  -subj '/CN=nanacoin-rs.local' \
  -addext 'subjectAltName=DNS:nanacoin-rs.local,DNS:localhost,IP:127.0.0.1'
echo 'Local development certificate created. Trust certs/server.crt on your client devices.'
