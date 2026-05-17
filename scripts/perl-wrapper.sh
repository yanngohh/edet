#!/bin/bash
# perl-wrapper.sh - Intercepts Perl execution in openssl-src to inject 'no-asm' configuration for cross-compilation target compatibility.

args=()
has_configure=false

for arg in "$@"; do
    args+=("$arg")
    if [[ "$arg" == *"Configure" ]]; then
        has_configure=true
        args+=("no-asm")
    fi
done

# If Configure is being executed, we inject no-asm to disable assembly routines
if [ "$has_configure" = true ]; then
    echo "💡 [perl-wrapper] Intercepted OpenSSL Configure call! Injecting 'no-asm' to bypass NDK assembler errors."
fi

exec perl "${args[@]}"
