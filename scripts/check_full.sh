#!/bin/bash
set -e

# Reports on the latest run started with scripts/run_full.sh: whether it is
# still running, where its files are, and the end of its output, which
# contains the summary once the run has finished. It returns at once.
HOST="${WORDLE_OPT_COMPUTE_HOST:?set WORDLE_OPT_COMPUTE_HOST to the remote machine, for example user@host}"
RUN_DIR="full-runs"

ssh "$HOST" "
	stampfile=\"\$HOME/$RUN_DIR/CURRENT\"
	if [ ! -f \"\$stampfile\" ]; then
		echo 'No full run has been started on this machine yet (no ~/$RUN_DIR/CURRENT).'
		exit 0
	fi
	stamp=\$(cat \"\$stampfile\")
	pidfile=\"\$HOME/$RUN_DIR/current.pid\"
	log=\"\$HOME/$RUN_DIR/\$stamp.log\"
	if [ -f \"\$pidfile\" ] && kill -0 \"\$(cat \"\$pidfile\")\" 2>/dev/null; then
		echo \"RUNNING (run \$stamp, pid \$(cat \"\$pidfile\"))\"
	else
		echo \"NOT RUNNING (run \$stamp has exited, see the output below)\"
		echo \"Files: ~/$RUN_DIR/\$stamp.json, ~/$RUN_DIR/\$stamp-progress.parquet, ~/$RUN_DIR/\$stamp.log\"
	fi
	echo \"--- last 25 lines of \$stamp.log ---\"
	tail -n 25 \"\$log\" 2>/dev/null || echo '(no output yet)'
"
