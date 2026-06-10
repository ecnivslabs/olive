def neg(x):
    return -x

xs = [5, 3, 8, 1, 9, 2]
ys = sorted(xs, key=neg)
print(ys)
