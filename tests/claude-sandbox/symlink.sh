# symlink: generate and show the profile
csb -f $SRC/symlink.conf generate
user_profile

echo "-- check"
csb -f $SRC/symlink.conf check $HOME/chain1/x $HOME/alias/y
