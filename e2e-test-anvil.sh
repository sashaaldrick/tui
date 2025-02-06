#!/bin/bash
set -eo pipefail

# Validate basic dependencies
command -v forge >/dev/null || { echo "Missing Foundry: install via foundup"; exit 1; }
command -v cargo >/dev/null || { echo "Install Rust toolchain: rustup.rs"; exit 1; }

# Environment detection and setup
if [[ -z "$BONSAI_API_KEY" ]]; then
  echo "Local mode: Starting Anvil chain..."
  if ! pgrep -x "anvil" >/dev/null; then
    anvil --mnemonic "test test test test test test test test test test test junk" --silent &
    ANVIL_PID=$!
    trap "kill $ANVIL_PID" EXIT
  fi
  
  export ETH_RPC_URL="http://localhost:8545"
  export ETH_WALLET_ADDRESS="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
  export ETH_WALLET_PRIVATE_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
fi

# Original test logic below
export TOKEN_OWNER=${ETH_WALLET_ADDRESS:?}
CHAIN_ID=$(cast rpc eth_chainId | jq -re)
CHAIN_ID=$((CHAIN_ID))

forge script --rpc-url ${ETH_RPC_URL:?} --private-key ${ETH_WALLET_PRIVATE_KEY:?} --broadcast DeployCounter

TOYKEN_ADDRESS=$(jq -re '.transactions[] | select(.contractName == "ERC20FixedSupply") | .contractAddress' ./broadcast/DeployCounter.s.sol/$CHAIN_ID/run-latest.json)
COUNTER_ADDRESS=$(jq -re '.transactions[] | select(.contractName == "Counter") | .contractAddress' ./broadcast/DeployCounter.s.sol/$CHAIN_ID/run-latest.json)
BLOCK_NUMBER=$(jq --arg ADDRESS "$TOYKEN_ADDRESS" -re '.receipts[] | select(.contractAddress == $ADDRESS) | .blockNumber' ./broadcast/DeployCounter.s.sol/$CHAIN_ID/run-latest.j