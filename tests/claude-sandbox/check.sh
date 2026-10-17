# check: which rule decides each access
csb -f $SRC/check.conf check \
    $HOME/.ssh/id_rsa \
    $HOME/.exrc \
    $HOME/work/root/foo \
    /vol-alias/work/root/foo \
    $HOME/Documents \
    /tmp/x \
    $HOME/.claude.json \
    $HOME/.claude.json.tmp.123 \
    $HOME/.claude/settings.json \
    $HOME \
    $HOME/work \
    $SRC/check.conf \
    $FIX/proj/file.c \
    /usr/bin/perl \
    /dev/null \
    /dev/ttys001

echo "-- no paths"
csb -f $SRC/check.conf check
echo "exit=$?"
