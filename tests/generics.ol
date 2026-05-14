struct Wrapper[T]:
    val: T

fn identity[T](x: T) -> T:
    return x

fn main():
    let w = Wrapper(1)
    print(w.val)

    let x = identity(42)
    print(x)

    let y = identity("hello")
    print(y)

main()
