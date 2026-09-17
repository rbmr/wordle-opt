#!/bin/bash
set -e

# Cheap, non-blocking status check for a full run started with run_full.sh.
# Takes under a second - use this instead of blocking a shell/agent session
# on the full run's completion. If it's still running, just check back
# later (e.g. re-run this script on your own schedule); don't sit and wait
# on it synchronously.
HOST="${WORDLE_OPT_COMPUTE_HOST:-robert@compute}"
LOG="wordle-opt/full_run.log"
PIDFILE="full_run.pid"

ssh "$HOST" "
	if [ ! -f $PIDFILE ]; then
		echo 'No full_run.pid found - run_full.sh has not been run yet (or the file was cleaned up).'
		exit 0
	fi
	pid=\$(cat $PIDFILE)
	if kill -0 \"\$pid\" 2>/dev/null; then
		echo \"RUNNING (pid \$pid)\"
	else
		echo 'NOT RUNNING (process has exited - check the tail below for completion or a crash)'
	fi
	echo '--- last 15 lines of $LOG ---'
	tail -n 15 $LOG 2>/dev/null || echo '(no log yet)'
"
