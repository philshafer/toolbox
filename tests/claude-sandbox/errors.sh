# errors: bad lines are reported with line numbers; no profile is written
SB_EMPTY= csb -f $SRC/errors.conf generate
echo "exit=$?"
ls "$TMPDIR"
