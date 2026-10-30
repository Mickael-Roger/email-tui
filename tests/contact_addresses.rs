use mailtui::{
    config::PimCollection,
    pim::{self, PostalAddress},
};
use std::fs;

#[test]
fn typed_vcard_addresses_roundtrip_without_losing_secondary_or_unknown_properties() {
    for version in ["3.0", "4.0"] {
        let root = tempfile::tempdir().unwrap();
        let book = PimCollection {
            id: "zimbra".into(),
            label: "Zimbra".into(),
            path: root.path().into(),
        };
        let file = root.path().join("server-name.vcf");
        let original = format!(
            "BEGIN:VCARD\r\nVERSION:{version}\r\nUID:contact\r\nFN:René\r\n\
            item1.ADR;TYPE=\"home,postal\";X-CUSTOM=keep:BP 4;Bâtiment A;12 rue\\; annexe\\nÉtage 2;Paris;Île-de-France;75001;France\r\n\
            ADR;TYPE=HOME:;;Secondary home;Nice;;06000;France\r\n\
            ADR;type=work:;;25 avenue du Port;Lyon;;69002;France\r\n\
            ADR:;;5 Main Street;Montréal;Québec;H2X 1Y4;Canada\r\n\
            X-UNKNOWN:preserve\r\nEND:VCARD\r\n"
        );
        fs::write(&file, &original).unwrap();
        let mut contact = pim::load(&[], std::slice::from_ref(&book))
            .contacts
            .remove(0);
        assert_eq!(contact.addresses[0].street, "12 rue; annexe\nÉtage 2");
        assert_eq!(contact.addresses[0].po_box, "BP 4");
        assert_eq!(contact.addresses[0].extended, "Bâtiment A");
        assert_eq!(contact.addresses[1].city, "Lyon");
        assert_eq!(contact.addresses[2].city, "Montréal");
        contact.name = "Dr. René".into();
        pim::write_contact(&book, &contact).unwrap();
        let saved = fs::read_to_string(&file).unwrap();
        for line in original.lines().filter(|line| line.contains("ADR")) {
            // Unfold output for RFC5545/vCard 75-byte line folding.
            assert!(saved.replace("\r\n ", "").contains(line), "Lost {line}");
        }
        let mut contact = pim::load(&[], std::slice::from_ref(&book))
            .contacts
            .remove(0);
        contact.addresses[0].street = "42 chemin, côté; cour\\entrée\nAppartement 3".into();
        contact.addresses[0].city = "Bordeaux".into();
        contact.addresses[1] = PostalAddress::default();
        contact.addresses[2].postal_code = "H2X 2Y2".into();
        pim::write_contact(&book, &contact).unwrap();
        let updated = pim::load(&[], std::slice::from_ref(&book))
            .contacts
            .remove(0);
        assert_eq!(updated.addresses, contact.addresses);
        let saved = fs::read_to_string(&file).unwrap().replace("\r\n ", "");
        assert!(saved.contains("item1.ADR;TYPE=\"home,postal\";X-CUSTOM=keep:BP 4;Bâtiment A;"));
        assert!(saved.contains("ADR;TYPE=HOME:;;Secondary home;Nice;;06000;France"));
        assert!(!saved.contains("ADR;type=work:"));
        assert!(saved.contains("X-UNKNOWN:preserve"));
        assert_eq!(updated.file, file);
    }
}

#[test]
fn absent_addresses_can_be_added_with_standard_types_and_cleared() {
    let root = tempfile::tempdir().unwrap();
    let book = PimCollection {
        id: "book".into(),
        label: "Book".into(),
        path: root.path().into(),
    };
    let file = root.path().join("contact.vcf");
    fs::write(
        &file,
        "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:contact\r\nFN:Ada\r\nEND:VCARD\r\n",
    )
    .unwrap();
    let mut contact = pim::load(&[], std::slice::from_ref(&book))
        .contacts
        .remove(0);
    for (kind, address) in contact.addresses.iter_mut().enumerate() {
        *address = PostalAddress {
            street: format!("{kind} rue des Lilas"),
            city: "Paris".into(),
            postal_code: "75001".into(),
            country: "France".into(),
            ..Default::default()
        };
    }
    pim::write_contact(&book, &contact).unwrap();
    let mut saved = pim::load(&[], std::slice::from_ref(&book))
        .contacts
        .remove(0);
    assert_eq!(saved.addresses, contact.addresses);
    let source = fs::read_to_string(&file).unwrap();
    for kind in ["HOME", "WORK", "OTHER"] {
        assert!(source.contains(&format!("ADR;TYPE={kind}:;;")));
    }
    saved.addresses = Default::default();
    pim::write_contact(&book, &saved).unwrap();
    assert!(!fs::read_to_string(file).unwrap().contains("ADR;"));
}
