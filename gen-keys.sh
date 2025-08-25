#!/bin/bash

mkdir -p keys keys/server keys/client
touch keys/client/tls-allowed-servers-certs.pem

openssl req -newkey rsa:4096 -nodes -keyout ./keys/server/tls-key.pem \
  -x509 -days 365 -out ./keys/server/tls-cert.pem \
  -subj "/CN=localhost" \
  -addext "subjectAltName = DNS:localhost" \
  -addext "basicConstraints=CA:FALSE" \
  -addext "keyUsage=digitalSignature,keyEncipherment" \
  -addext "extendedKeyUsage=serverAuth"

openssl x509 -noout -modulus -in ./keys/tls-cert.pem | openssl md5
openssl rsa -noout -modulus -in ./keys/tls-key.pem | openssl md5