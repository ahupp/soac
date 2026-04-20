class Record:
    def __init__(self, x=0, y=0):
        self.x = x
        self.y = y

    def copy(self):
        return Record(self.x, self.y)


def run():
    record = Record(1, 2)
    record.x = 3
    copied = record.copy()
    return copied.x + copied.y + record.x
