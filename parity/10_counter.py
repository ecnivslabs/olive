from collections import Counter

xs = [1, 2, 2, 3, 3, 3, 4]
c = Counter(xs)
top = c.most_common(2)
print(top[0])
print(top[1])
