# self  ( -- obj )
#     Pushes the current message receiver from user_SELF.
#     At top level (outside any method) the receiver is 0.
call self_word
expect 0
