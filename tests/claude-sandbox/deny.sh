# deny: generate, then check the holes
csb -f $SRC/deny.conf generate
user_profile

echo "-- check"
csb -f $SRC/deny.conf check \
    /tmp/logs/a \
    /tmp/secret/b \
    /tmp/other \
    $HOME/bin/tool \
    $HOME/bin/private/x \
    $HOME/Documents/y \
    $HOME/.claude/private/z \
    $HOME/.claude/public
