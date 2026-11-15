#!/usr/bin/env bash
# Fails when the repository tracks files that must never be public: invites with a join
# token, backups, databases, keys and host secrets.
set -euo pipefail

status=0

blocked=$(git ls-files | grep -E '\.(lanvox-backup|sqlite3?|db|key|secret|invite)$|(^|/)host-config\.json$|(^|/)invite[^/]*\.txt$|\.log$' || true)
if [ -n "$blocked" ]; then
  echo "::error::Tracked files that must stay private:"
  echo "$blocked"
  status=1
fi

# A real invite carries a token after the address; tests and docs use placeholders.
leaks=$(git grep -n -I -E 'lanvox://[^ ]*[?&]token=[A-Za-z0-9_-]{20,}' -- . ':!.github/scripts/check-secrets.sh' || true)
if [ -n "$leaks" ]; then
  echo "::error::Possible invite with a real-looking token:"
  echo "$leaks"
  status=1
fi

exit $status
