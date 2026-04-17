

Say we have a very simple function:

def add(a, b):
    c = a + b
    if c > 0:
       return True


In an IR form this looks like:

c = BinOp(Add, a, b)
tmp1 = BinOp(Gt, c, 0)
tmp2 = Truthy(tmp1)
if tmp2:
  return True


Because Python, this can do lots of things:

 * Maybe type(a).__add__ always returns "5"
 * Maybe type(c).__gt__ returns "yes" and "no"
 * Maybe type(tmp2).__bool__ makes a network request.
 * Or maybe these are all ints and bools and work as you'd expect


Our mission is to turn this into the most optimal specialized
instruction sequence *for our workload*.  What do we know about our
workload?  We profile against the raw unoptimized instructions above.
Maybe tells us that:

 1. At entry, most of the time, `a` and `b` are exact PyLong, and
   compact (can be represented in an i64).
 2 Sometimes `b` is a user-defined type `Weird` that has an `__radd__` 


So after dispatch, the specific sequence of python operations is most often:

def add(a, b):
  a : Borrowed[object]
  b : Borrowed[object]
  if type(a) is int and type(b) is int:
    a : Borrowed[int]
    b : Borrowed[int]
    c : Owned[int] = int.__add__(a, b)
    tmp1 : Immortal[bool] = int.__gt__(c, 0)
    tmp2 : Immortal[bool] = bool.__bool__(tmp1)
    if tmp2:
      ret : Immortal[bool] = True
    else:
      ret : Immortal[None] = None
    return ret
  else:
   ...

We happen to know a lot about those functions:

  * they have no side-effects
  * none of them will throw (since the types are checked on entry)
  * bool.__bool__ is the identity function


Issue: here our specialization depends on two variables; how do we
know when those should be treated as one specialization, vs two
partial?

Some possibilities: 
 -by default we specialization on the full set of argument types?
 - or, we determine the "entry" point is BinOp(Add, a, b) and select all variables named

Avoiding the dispatch to figure out to use int.__add__ is nice, but we can do better:

If both arguments are also "compact" integers, they can be represneted as machine ints:


def add(a, b):
  a : Borrowed[object]
  b : Borrowed[object]
  if type_is_compact_int(a) and type_is_compact_int(b):
    a_m = extract_machine_int(a)
    b_m = extract_machine_int(b)
    c = add_i64(a_m, b_m)
    tmp = gt_i64(c, 0)
    if tmp:
      ret : Immortal[bool] = True
    else:
      ret : Immortal[None] = None
    return ret
  else:
   ...
