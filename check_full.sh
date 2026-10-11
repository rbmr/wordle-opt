#!/bin/bash
set -e

# Cheap, non-blocking status check for a full run started with run_full.sh.
# Takes under a second - use this instead of blocking a shell/agent session
# on the full run's completion. If it's still running, just check back
# later (e.g. re-run this script on your own schedule); don't sit and wait
# on it synchronously.
#
# The run's artifacts are stamped with the UTC day and the commit, so this
# prints the stamp it is reporting on and where that run's files are. When a
# run has finished, the tail includes its summary block - copy the row from
# there into run_history.md.
HOST="${WORDLE_OPT_COMPUTE_HOST:-user@pc01}"
RUN_DIR="full-runs"

ssh "$HOST" "
	stampfile=\"\$HOME/$RUN_DIR/CURRENT\"
	if [ ! -f \"\$stampfile\" ]; then
		echo 'No full run has been started on this host yet (no ~/$RUN_DIR/CURRENT).'
		exit 0
	fi
	stamp=\$(cat \"\$stampfile\")
	pidfile=\"\$HOME/$RUN_DIR/current.pid\"
	log=\"\$HOME/$RUN_DIR/\$stamp.log\"
	if [ -f \"\$pidfile\" ] && kill -0 \"\$(cat \"\$pidfile\")\" 2>/dev/null; then
		echo \"RUNNING (run \$stamp, pid \$(cat \"\$pidfile\"))\"
	else
		echo \"NOT RUNNING (run \$stamp has exited - see the tail below for completion or a crash)\"
		echo \"Artifacts: ~/$RUN_DIR/\$stamp.json, ~/$RUN_DIR/\$stamp-progress.parquet, ~/$RUN_DIR/\$stamp.log\"
	fi
	echo \"--- last 15 lines of \$stamp.log ---\"
	tail -n 15 \"\$log\" 2>/dev/null || echo '(no log yet)'
"
