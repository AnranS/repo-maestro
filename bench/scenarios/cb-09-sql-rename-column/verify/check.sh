#!/usr/bin/env sh
set -eu
test -f producer/migration.sql || exit 1
