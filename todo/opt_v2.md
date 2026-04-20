The goal of this exercise is to plan a system that:

 - can represent all of the optimizating transforms of this python runtiem
 - in a way that uniformly captures the "cost" of each, and is
   amenable to offline optimziation
 - And has a reasonably high chance of finding the "optimal"
   instruction sequence for an equivalent program.

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
instruction sequence *for our workload*.  

By default, this ends up running something like:

def add(a, b):

  c = PyNumber_Add(a, b)
  tmp1 = PyObject_RichCompare(c, 0, Py_GT);
  tmp2 = PyObject_IsTrue(tmp1);
  if tmp2:
    return True

We happen to know what these functions do, and if you look at each
call in isolation an equivalent form is:


if type(a) is PyLong and type(b) is PyLong:
  c = PyLong.__add__(a, b)
else:
  c = PyNumber_Add(a, b)

if type(c) is PyLong:
  tmp1 = PyLong.__gt__(c, 0)
else:
  tmp1 = PyObject_RichCompare(c, 0, Py_GT)

if type(tmp1) is bool:
  tmp2 = bool.__bool__(tmp1)
else:
  tmp2 = PyObject_IsTrue(tmp1)


That is probably nnot any faster, but it is explicit.  We can do
better.  If you instead track the most common *sequence* of calls, it
would look like this:

if type(a) is PyLong and type(b) is PyLong:
  c = PyLong.__add__(a, b)

  if type(c) is PyLong:
      tmp1 = PyLong.__gt__(c, 0)
  else:
      tmp1 = PyObject_RichCompare(c, 0, Py_GT)

   if type(tmp1) is bool:
      tmp2 = bool.__bool__(tmp1)
   else:
      tmp2 = PyObject_IsTrue(tmp1)
else:
  c = PyNumber_Add(a, b)
  tmp1 = PyObject_RichCompare(c, 0, Py_GT)
  tmp2 = PyObject_IsTrue(tmp1)

Which seems worse, except now we know that PyLong.__add__ will always
return PyLong, and PyLong.__gt__ will always return bool:

if type(a) is PyLong and type(b) is PyLong:
  c = PyLong.__add__(a, b)

  tmp1 = PyLong.__gt__(c, 0)
  tmp2 = bool.__bool__(tmp1)
else:
  c = PyNumber_Add(a, b)
  tmp1 = PyObject_RichCompare(c, 0, Py_GT)
  tmp2 = PyObject_IsTrue(tmp1)

We also know bool.__bool__ is `arg is True` if the argument is bool:

if type(a) is PyLong and type(b) is PyLong:
  c = PyLong.__add__(a, b)
  tmp1 = PyLong.__gt__(c, 0)
  tmp2 = tmp1 is True
else:
  c = PyNumber_Add(a, b)
  tmp1 = PyObject_RichCompare(c, 0, Py_GT)
  tmp2 = PyObject_IsTrue(tmp1)


We also know that the `if` stmt terminating all this has a "demand"
for Int01, eg a bool.  To bring the cost down as low as possible, we
need to decide that this should be:

if is_compact_int(a) and is_compact_int(b):
  c = as_compact_int(a) + as_compact_int(b)
  tmp2 = c > 0
else:
  c = PyNumber_Add(a, b)
  tmp1 = PyObject_RichCompare(c, 0, Py_GT)
  tmp2 = PyObject_IsTrue(tmp1)

Note the previous steps were fairly mechanical, but there was a bit of
a leap at the last one.

Each of these forms should have a "cost" estimated, based on a)
straight-line time for each form based on expected time, b) "space",
in code size, and c) an overll time, based on the impact of the added
code size.  Note that someteimes step N will be higher cost, but
applying step N+1 will make it lower cost.  Cost should also reflect
any changes in memory management time (e.g, refcounts, and
allocations).




We have a few built-in facts about the world, like that these functions exist:

add(a: [exact(PyLong), Borrowed],
    b: [exact(PyLong), Borrowed]) -> [exact(PyLong), Owned] {
 /// PyLong_Add(...)
}

add(a: [machine(i64), machine(i64)]) -> [machine(i64)] {
  ret : i64 = compact_add_i64(a, b);
  if did_overflow():
    // We might also choose to opt a module into strictly compact ints, 
    // throwing rather than deopting
    deopt;
  }
  return ret;
}

add(a: [PyObject, Borrowed], b: [PyObject, Borrowed]) -> [PyObject, Owned] {
 // PyNumber_Add, default case
}

And conversions:

convert(a: [PyLong, Borrowed, IsCompact]) -> i64 {
  
}

// Infalliable
convert(a: [i64]) -> PyLong {...}

And each of these functions has an associated cost estimated.  When a
function has a "deopt" term, it guaruntes no side effects up to that
point in the function.

Some open questions:

 - how do we use profiling to record the "traces" expected here?
 - how / when do we do "linearization", to hoist subexpressions and make complex expr only refer to local variables?
 - how do we efficiently search the space of transforms?

