# 10*  ( n -- 10n )

push 8
call ten_times
expect 80

reset
push -9
call ten_times
expect -90