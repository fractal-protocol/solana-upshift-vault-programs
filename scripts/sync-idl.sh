#!/bin/bash

# Script to sync IDL files from contract build to app directory
# Run this after 'anchor build' to update the app with latest IDL

echo "🔄 Syncing IDL files to app directory..."

# Create frontend/idl directory if it doesn't exist
mkdir -p frontend/idl

# Copy IDL files
if [ -f "target/idl/august_vault.json" ]; then
    cp target/idl/august_vault.json frontend/idl/
    echo "✅ Copied august_vault.json"
else
    echo "❌ target/idl/august_vault.json not found. Run 'anchor build' first."
    exit 1
fi

if [ -f "target/types/august_vault.ts" ]; then
    cp target/types/august_vault.ts frontend/idl/
    echo "✅ Copied august_vault.ts"
else
    echo "❌ target/types/august_vault.ts not found. Run 'anchor build' first."
    exit 1
fi

echo "🎉 IDL sync complete!"
