#!/bin/sh
# Fresh self-signed certificate per start: FTPS clients see an unknown
# certificate every run (exercises the trust-on-first-use flow).
set -e
: "${FTP_USER:=cx}" "${FTP_PASS:=cxpass}" "${PUBLICHOST:=127.0.0.1}" "${PASSIVE_PORTS:=30000:30019}"

mkdir -p /etc/ssl/private "/home/$FTP_USER"
openssl req -x509 -nodes -newkey rsa:2048 -days 30 -subj "/CN=localhost" \
  -keyout /etc/ssl/private/pure-ftpd.pem -out /etc/ssl/private/pure-ftpd.pem 2>/dev/null
chmod 600 /etc/ssl/private/pure-ftpd.pem
chown ftpuser "/home/$FTP_USER"

rm -f /etc/pureftpd.passwd /etc/pureftpd.pdb
printf '%s\n%s\n' "$FTP_PASS" "$FTP_PASS" | \
  pure-pw useradd "$FTP_USER" -u ftpuser -d "/home/$FTP_USER" -f /etc/pureftpd.passwd -m -F /etc/pureftpd.pdb >/dev/null

# -E no anonymous  -j create home  -Y 1 TLS optional (explicit)
# -P passive address  -p passive ports  -c/-C client limits
exec pure-ftpd -l puredb:/etc/pureftpd.pdb -E -j -Y 1 \
  -P "$PUBLICHOST" -p "$PASSIVE_PORTS" -c 50 -C 50
