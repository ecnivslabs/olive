import re

s = "cat cot cut bat"
found = re.findall(r"c[aeiou]t", s)
print(len(found))
print("-".join(found))
