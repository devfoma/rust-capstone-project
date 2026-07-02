# Setup nvm and install pre-req
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.39.1/install.sh | bash
nvm install --lts
npm install

set -e  # Exit immediately if any command fails

# Spawn Bitcoind, and provide execution permission.
docker compose up -d
sleep 10

echo "Waiting for bitcoind to be fully initialized..."

while true; do
  result=$(curl --silent --user alice:password --data-binary \
    '{"jsonrpc":"1.0","id":"ping","method":"getblockchaininfo","params":[]}' \
    -H 'content-type: text/plain;' http://127.0.0.1:18443)

  if echo "$result" | grep -q '"chain"'; then
    echo "bitcoind is ready."
    break
  else
    echo "bitcoind not ready yet, retrying in 3s..."
    sleep 3
  fi
done

chmod +x ./rust/run-rust.sh
chmod +x ./run.sh

# Run the test scripts
/bin/bash run.sh
npm run test

# Stop the docker.
docker compose down -v