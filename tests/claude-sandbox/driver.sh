#!/bin/sh
#
# driver.sh NAME -- run one claude-sandbox test (NAME.sh), leaving
# its output in out/NAME.out and out/NAME.err
#
# Each test runs against a fresh fixture under out/fix/NAME: a fake
# $HOME, a workspace ($CWD), a $TMPDIR, and a fake "claude" binary.
# Absolute paths are rewritten to @FIX@ and @SRC@ so the output does
# not depend on where the repo lives.
#

t=$1
if [ -z "$t" -o ! -f "`dirname $0`/$t.sh" ]; then
    echo "usage: driver.sh test-name" 1>&2
    exit 1
fi

SRC=`cd \`dirname $0\` && pwd -P`
OUT=$SRC/out
FIX=$OUT/fix/$t
SCRIPT=`cd $SRC/../../claude-sandbox && pwd -P`/claude-sandbox.pl

rm -rf $FIX
mkdir -p $FIX/home $FIX/proj/sub $FIX/var/T $FIX/real $FIX/vol/work/root \
    $FIX/dest

# ---------- the fake home directory -------------------------------
H=$FIX/home
mkdir -p $H/.ssh $H/.claude $H/.local/bin $H/.local/share $H/bin/private \
    $H/Documents $H/Library/Preferences $H/patches/claude "$H/My Docs"
touch $H/.exrc $H/.vimrc $H/.ssh/id_rsa $H/.claude.json $H/bin/tool \
    $FIX/proj/.lldbinit
printf '#!/bin/sh\necho fake claude\n' > $H/.local/bin/claude
chmod +x $H/.local/bin/claude

# ~/work is a relative symlink onto another "volume"
ln -s ../vol/work $H/work

# chain1 -> chain2 -> $FIX/real
ln -s chain2 $H/chain1
ln -s $FIX/real $H/chain2

# a symlink loop
ln -s loop2 $H/loop1
ln -s loop1 $H/loop2

# ---------- environment -------------------------------------------
HOME=$H
TMPDIR=$FIX/var/T/
PATH=$H/.local/bin:/usr/bin:/bin
export HOME TMPDIR PATH
unset SB_DESTDIR SB_PROJ SB_EMPTY SB_UNSET

cd $FIX/proj

# Run the script under test
csb() {
    perl "$SCRIPT" "$@"
}

# Print the generated profile, minus the date and the system runtime
# section (which depends on what is installed on this machine)
profile() {
    awk '/^;;  generated:/ { next }
         /^;; ---------- system runtime/ { skip = 1 }
         skip && /^$/ { skip = 0; next }
         !skip { print }' "$TMPDIR"claude-sandbox-*.sb
}

# Print just the part of the profile that comes from the conf file:
# from the $CWD rule through the forced denies
user_profile() {
    awk '/^;; cwd:1:/ { on = 1 }
         /^;; ---------- mach-lookup/ { on = 0 }
         on { print }' "$TMPDIR"claude-sandbox-*.sb
}

# Rewrite machine-specific paths
filter() {
    sed -e "s|$FIX|@FIX@|g" \
        -e "s|$SRC|@SRC@|g" \
        -e 's|claude-sandbox-proj-[0-9a-f]*\.sb|claude-sandbox-proj-HASH.sb|g'
}

( . $SRC/$t.sh ) > $OUT/$t.out.raw 2> $OUT/$t.err.raw

filter < $OUT/$t.out.raw > $OUT/$t.out
filter < $OUT/$t.err.raw > $OUT/$t.err
rm -f $OUT/$t.out.raw $OUT/$t.err.raw
