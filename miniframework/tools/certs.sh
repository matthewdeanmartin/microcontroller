#!/usr/bin/env bash
# Usage: bash tools/certs.sh <app-dir> <name>
#
# Issues an HTTPS server certificate for <name>.local, signed by the
# household CA that mastomini created, so devices that already trust
# mastomini (and NanaCoin) trust this board too. No new CA, ever.
#
# Writes into <app-dir>/certs/:
#   <name>.crt, <name>.key   server certificate and key (key is gitignored)
#   household-ca.crt, .der   the CA certificate (public; served at /ca)
#
# The CA's private key is only read, never copied. Where it is:
#   MINIFRAMEWORK_CA_DIR   (default: ../../mastomini/mastomini_rs/.local/ca
#                           relative to miniframework/, holding rootCA.pem
#                           and rootCA-key.pem)
# Extra names / addresses in the certificate (comma separated):
#   MINIFRAMEWORK_CERT_NAMES, MINIFRAMEWORK_CERT_IPS (e.g. the board's IP)
#
# Existing files are never replaced. To reissue (new IP, nearing expiry),
# move the old pair away first; devices keep trusting the same CA.
set -euo pipefail
[[ $# -eq 2 ]] || { echo 'Usage: bash tools/certs.sh <app-dir> <name>' >&2; exit 2; }
root=$(cd "$(dirname "$0")/.." && pwd)
app=$(cd "$1" && pwd)
name=$2
host="$name.local"
command -v openssl >/dev/null || { echo 'Install OpenSSL first (Git for Windows includes it).' >&2; exit 1; }

# Native openssl on Windows needs C:/... paths.
native() { if command -v cygpath >/dev/null; then cygpath -m "$1"; else printf '%s' "$1"; fi; }
ca_dir=${MINIFRAMEWORK_CA_DIR:-$root/../../mastomini/mastomini_rs/.local/ca}
root_cert=$ca_dir/rootCA.pem
root_key=$ca_dir/rootCA-key.pem
out=$app/certs
leaf=$out/$name.crt
key=$out/$name.key
mkdir -p "$out"

if [[ -f $leaf && -f $key ]]; then
  echo "Server certificate already exists: $leaf (not replaced)"
elif [[ -e $leaf || -e $key ]]; then
  echo "Only one of $leaf / $key exists; refusing to guess. Move it away and rerun." >&2
  exit 1
else
  [[ -f $root_cert && -f $root_key ]] || {
    echo "No household CA in $ca_dir (rootCA.pem + rootCA-key.pem)." >&2
    echo 'Set MINIFRAMEWORK_CA_DIR to the CA mastomini uses. This script never creates a CA:' >&2
    echo 'a second CA would mean trusting yet another certificate on every device.' >&2
    exit 1
  }
  names=("$host" localhost)
  ips=(127.0.0.1)
  IFS=',' read -r -a extra <<<"${MINIFRAMEWORK_CERT_NAMES:-}"
  for n in "${extra[@]}"; do n=${n// /}; [[ -z $n ]] || names+=("$n"); done
  IFS=',' read -r -a extra <<<"${MINIFRAMEWORK_CERT_IPS:-}"
  for n in "${extra[@]}"; do n=${n// /}; [[ -z $n ]] || ips+=("$n"); done
  san=""
  for n in "${names[@]}"; do san+="${san:+,}DNS:$n"; done
  for n in "${ips[@]}"; do san+=",IP:$n"; done

  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  : >"$work/index.txt"
  cat >"$work/ca.cnf" <<EOF
[ca]
default_ca = household
[household]
database = $(native "$work/index.txt")
new_certs_dir = $(native "$work")
rand_serial = yes
default_md = sha256
policy = anything
unique_subject = no
email_in_dn = no
[anything]
organizationName = optional
organizationalUnitName = optional
commonName = supplied
EOF
  cat >"$work/leaf.ext" <<EOF
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=$san
subjectKeyIdentifier=hash
authorityKeyIdentifier=keyid,issuer
EOF
  # RSA-2048: mbedTLS on the board and every browser handle it well.
  # Starts a day early (devices showing UTC, clocks running behind) and
  # lasts 820 days in all: Apple rejects longer-lived server certificates
  # even from a CA the user installed.
  start=$(( $(date -u +%s) - 86400 ))
  stamp() { date -u -d "@$1" +%Y%m%d%H%M%SZ; }
  (umask 077; MSYS2_ARG_CONV_EXCL='*' openssl req -quiet -new -newkey rsa:2048 -sha256 -nodes \
    -keyout "$(native "$key")" -out "$(native "$work/leaf.csr")" -subj "/O=miniframework household/CN=$host")
  MSYS2_ARG_CONV_EXCL='*' openssl ca -batch -notext -preserveDN -config "$(native "$work/ca.cnf")" \
    -in "$(native "$work/leaf.csr")" -out "$(native "$leaf")" \
    -startdate "$(stamp "$start")" -enddate "$(stamp $(( start + 820 * 86400 )))" \
    -extfile "$(native "$work/leaf.ext")" -cert "$(native "$root_cert")" -keyfile "$(native "$root_key")" \
    2>"$work/ca.log" || { cat "$work/ca.log" >&2; rm -f "$key" "$leaf"; exit 1; }
  echo "Issued $leaf for: ${names[*]} ${ips[*]}"
fi

# The CA certificate the board serves at /ca: always the one that signed the leaf.
issuer_cert=$root_cert
[[ -f $issuer_cert ]] || issuer_cert=$out/household-ca.crt
[[ -f $issuer_cert ]] || { echo "No CA certificate to publish." >&2; exit 1; }
openssl verify -CAfile "$(native "$issuer_cert")" "$(native "$leaf")" >/dev/null || {
  echo "$leaf is not signed by $issuer_cert" >&2
  exit 1
}
[[ $issuer_cert == "$out/household-ca.crt" ]] || cp "$issuer_cert" "$out/household-ca.crt"
openssl x509 -in "$(native "$out/household-ca.crt")" -outform DER -out "$(native "$out/household-ca.der")"
# The key must belong to the certificate.
[[ "$(openssl x509 -in "$(native "$leaf")" -noout -pubkey)" == "$(openssl pkey -in "$(native "$key")" -pubout)" ]] || {
  echo "$key does not match $leaf" >&2
  exit 1
}
echo "Valid until $(openssl x509 -in "$(native "$leaf")" -noout -enddate | sed 's/notAfter=//')"
echo "Household CA SHA-256: $(openssl x509 -in "$(native "$out/household-ca.crt")" -noout -fingerprint -sha256 | sed 's/^.*=//')"
