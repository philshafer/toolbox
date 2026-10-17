# env: environment variables with ${NAME}
SB_DESTDIR=$FIX/dest SB_PROJ=foo csb -f $SRC/env.conf generate
user_profile

echo "-- check"
SB_DESTDIR=$FIX/dest SB_PROJ=foo csb -f $SRC/env.conf check \
    /dest-alias/x \
    $FIX/dest/log.1
