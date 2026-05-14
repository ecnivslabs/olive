struct Box[T]:
    val: T
    fn __init__(self, x: T):
        self.val = x

fn main():
    let b = Box(123)
    print(b.val)

main()
