def get_item(obj, index):
    return obj[index]


class PlainListSubclass(list):
    pass


class OverriddenListSubclass(list):
    def __getitem__(self, index):
        return ("override", index)


# diet-python: validate
assert get_item([10, 20, 30], 1) == 20
assert get_item([10, 20, 30], -1) == 30
assert get_item(PlainListSubclass([40, 50, 60]), 2) == 60
assert get_item(OverriddenListSubclass([70, 80, 90]), 1) == ("override", 1)
