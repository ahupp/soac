Currently, "instrinc" functions like `__dp_add` are defined as plain strings and
represented as `BlockPyExpr::Call` through the pipeline.  I'd like to carry this in a more structured way, to make codegen easier and
allow for more type-aware optimizations later on.

I'm thinking a trait like:

```
trait Intrinsic {
    type Args;
    type Ret;

    fn emit(&self, ...?);
}

struct IntrinsicCall<I: Intrinsic> {

  call: I,
  arg_bindings: tuple of same length as I::Args,

}
```


```
impl Intrinsic2: Intrinsic {}

impl Intrinsic {
    fn emit(&self, ....?);
}

fn call2<I: Intrinsic2>(intrinsic: I, arg0: ) -> Call {
    Call {
        intrinsic: Box::new(intrinsic),
        args: vec![arg0, arg1],
    }
}

call2(Add, )
```