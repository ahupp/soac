def set_item(obj, index, value):
    obj[index] = value
    return obj


class PlainListSubclass(list):
    pass


class OverriddenListSubclass(list):
    def __setitem__(self, index, value):
        super().__setitem__(index, 100 + value)


# diet-python: validate
first = [10, 20, 30]
set_item(first, 1, 99)
assert first == [10, 99, 30]

second = [1, 2, 3]
set_item(second, -1, 7)
assert second == [1, 2, 7]

plain = PlainListSubclass([40, 50, 60])
set_item(plain, 2, 80)
assert plain == [40, 50, 80]

overridden = OverriddenListSubclass([1, 2, 3])
set_item(overridden, 1, 5)
assert overridden == [1, 105, 3]
