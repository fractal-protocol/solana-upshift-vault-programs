#!/bin/bash

# Script to sync IDL files from contract build to app directory
# Run this after 'anchor build' to update the app with latest IDL

echo "🔄 Syncing IDL files to app directory..."

# Create frontend/idl directory if it doesn't exist
mkdir -p frontend/idl

# Copy IDL files
for program in august_vault august_withdrawal_queue; do
    if [ -f "target/idl/${program}.json" ]; then
        cp "target/idl/${program}.json" frontend/idl/
        echo "✅ Copied ${program}.json"
    else
        echo "❌ target/idl/${program}.json not found. Run 'anchor build' first."
        exit 1
    fi
done

for program in august_vault august_withdrawal_queue; do
    if [ -f "target/types/${program}.ts" ]; then
        cp "target/types/${program}.ts" frontend/idl/
        echo "✅ Copied ${program}.ts"
    else
        echo "❌ target/types/${program}.ts not found. Run 'anchor build' first."
        exit 1
    fi
done

echo "🎉 IDL sync complete!"
