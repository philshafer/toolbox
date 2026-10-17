# optional: "?" paths
echo "-- SB_DESTDIR unset"
SB_PROJ=proj SB_EMPTY= csb -f $SRC/optional.conf generate
user_profile

echo "-- SB_DESTDIR set"
SB_DESTDIR=$FIX/dest SB_PROJ=proj SB_EMPTY= csb -f $SRC/optional.conf generate
user_profile

echo "-- without \"?\", unset is still an error"
echo '+rwx ${SB_UNSET}' > plain.conf
csb -f plain.conf generate
echo "exit=$?"

echo "-- \"?\" does not excuse a bad \${"
echo '+rwx ?${SB_OPEN' > open.conf
csb -f open.conf generate
echo "exit=$?"
