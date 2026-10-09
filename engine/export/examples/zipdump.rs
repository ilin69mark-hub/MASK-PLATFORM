fn main() {
    let entries = vec![
        mask_export::archive::Entry {
            name: "a.svg",
            data: b"one",
        },
        mask_export::archive::Entry {
            name: "b.dxf",
            data: b"two",
        },
        mask_export::archive::Entry {
            name: "c.json",
            data: b"three",
        },
    ];
    let z = mask_export::archive::zip(&entries, mask_export::archive::DOS_EPOCH);
    std::fs::write("/tmp/probe3.zip", &z).unwrap();
    println!("{} байт", z.len());
    let tail = &z[z.len() - 22..];
    println!("EOCD хвост: {:02x?}", tail);
}
