//! Recursive descriptors retain their original-root back-reference context.

#[path = "support/program.rs"]
mod program;
use program::assert_both;

#[test]
fn any_struct_formatter_follows_recursive_backrefs() {
    assert_both(
        r#"struct Node:
    value: str
    next: Node | None

fn main():
    let node: Any = Node("root", Node("leaf", None))
    print(node)
"#,
        "Node(value=\"root\", next=Node(value=\"leaf\", next=None))\n",
    );
}

#[test]
fn recursive_tuple_narrow_keeps_following_field_aligned() {
    assert_both(
        r#"struct Node:
    value: str
    next: Node | None

fn use_it(value: (Node, int) | int):
    match value:
        case 0:
            print("none")
        case pair:
            print(pair[0].value)
            print(pair[0].next?.value)
            print(pair[1])

fn main():
    use_it((Node("root", Node("leaf", None)), 7))
"#,
        "root\nleaf\n7\n",
    );
}

#[test]
fn nested_enum_formats_recursive_payload() {
    assert_both(
        r#"struct Node:
    value: str
    next: Node | None

enum E:
    V(Node)

fn main():
    let value: Any = (V(Node("root", Node("leaf", None))), 3)
    print(value[0])
    print(value[1])
"#,
        "V(Node(value=\"root\", next=Node(value=\"leaf\", next=None)))\n3\n",
    );
}
