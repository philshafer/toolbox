# options: -f handling
echo "-- -f without a file"
csb -f
echo "exit=$?"

echo "-- relative -f is relative to the current directory"
echo "+rw rel-dir" > rel.conf
csb -f rel.conf check $HOME/rel-dir/x

echo "-- the last -f wins"
echo "+rw first-dir" > first.conf
csb -f first.conf -f rel.conf check $HOME/first-dir/x $HOME/rel-dir/x

echo "-- profile prints the path without generating"
csb profile
