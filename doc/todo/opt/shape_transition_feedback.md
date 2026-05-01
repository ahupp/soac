---
title: "Shape Transition Feedback"
---

# Shape Transition Feedback

## Article Spark

V8 hidden classes form transition trees. Consistent property insertion order produces shared maps;
different insertion order produces different maps even when the final property set is the same.

## SOAC Question

Can SOAC record type/module/instance key-layout transitions so it can distinguish "stable growing
prefix" from "arbitrary dict churn"?

## Concrete Experiment

- For watched split-key/type/module key objects, record layout insertion events as transitions:
  parent layout identity, inserted key, key index, owning module/type identity, and count.
- In counter dumps, preserve the transition tree or a compact parent pointer per layout.
- Use transition data to select subtype/base-prefix field fast paths: guard that runtime layout is a
  descendant of the expected base layout and use the base index.
- Flag layouts that differ only by insertion order, so benchmark tooling can recommend constructor
  normalization.

## Success Signal

- Specialization can keep a base-class field fast path valid for single-inheritance subclasses whose
  layout extends the base prefix.
- Counter inspection can explain why a field site is polymorphic: true different owners versus same
  key set in different insertion order.

## Risks

- Existing CPython split-dict key sharing already has subtle mutation/promotion rules; feedback must
  not keep dead key objects alive accidentally.
