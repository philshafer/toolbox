# oneline: -x writes one rule line per path (basic.conf, plus a regex)
csb -x -f $SRC/basic.conf generate
user_profile

echo "-- -x after -f, with the full profile"
csb -f $SRC/quoting.conf -x generate
profile
