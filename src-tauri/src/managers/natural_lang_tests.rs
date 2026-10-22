//! Fixed-outcome tests for the BEST-EFFORT English detector that gates the Phonon route
//! (DESIGN.md Rev 4, P4 and section 5). The detector is NOT a language identifier: the
//! contract is "route to Phonon unless foreign evidence outweighs English evidence".
//! These are the authoritative spec: do not weaken or delete assertions.

use super::natural_chunk::is_probably_english;

fn assert_all(expected: bool, sentences: &[&str], label: &str) {
    let wrong: Vec<&&str> = sentences
        .iter()
        .filter(|s| is_probably_english(s) != expected)
        .collect();
    assert!(
        wrong.is_empty(),
        "{label}: expected {expected} for all, but these differ: {wrong:?}"
    );
}

#[test]
fn english_corpus_is_all_english() {
    assert_all(
        true,
        &[
            "Sure, I can help you with that.",
            "The build finished successfully and all tests passed.",
            "Your meeting with the design team starts in ten minutes.",
            "I found three files that match your search.",
            "Would you like me to read the summary out loud?",
            "It is currently sixty degrees and cloudy outside.",
            "Okay, I have added the reminder to your calendar.",
            "There was an error while saving the document.",
            "Let me know if you want to change anything.",
            "Done.",
        ],
        "english",
    );
}

#[test]
fn spanish_corpus_is_all_foreign() {
    assert_all(
        false,
        &[
            "Claro, puedo ayudarte con eso.",
            "La compilación terminó correctamente y todas las pruebas pasaron.",
            "Tu reunión con el equipo de diseño empieza en diez minutos.",
            "Encontré tres archivos que coinciden con tu búsqueda.",
            "¿Quieres que lea el resumen en voz alta?",
            "Ahora hace sesenta grados y está nublado afuera.",
            "Vale, he añadido el recordatorio a tu calendario.",
            "Hubo un error al guardar el documento.",
        ],
        "spanish",
    );
}

#[test]
fn french_corpus_is_all_foreign() {
    assert_all(
        false,
        &[
            "Bien sûr, je peux vous aider avec cela.",
            "La compilation s'est terminée avec succès et tous les tests ont réussi.",
            "Votre réunion avec l'équipe de conception commence dans dix minutes.",
            "J'ai trouvé trois fichiers qui correspondent à votre recherche.",
            "Voulez-vous que je lise le résumé à voix haute ?",
            "Il fait actuellement quinze degrés et le ciel est couvert.",
            "D'accord, j'ai ajouté le rappel à votre calendrier.",
            "Une erreur s'est produite lors de l'enregistrement du document.",
        ],
        "french",
    );
}

#[test]
fn german_corpus_is_all_foreign() {
    assert_all(
        false,
        &[
            "Natürlich, ich kann Ihnen dabei helfen.",
            "Der Build wurde erfolgreich abgeschlossen und alle Tests sind bestanden.",
            "Ihr Treffen mit dem Designteam beginnt in zehn Minuten.",
            "Ich habe drei Dateien gefunden, die zu Ihrer Suche passen.",
            "Möchten Sie, dass ich die Zusammenfassung vorlese?",
            "Es ist draußen gerade fünfzehn Grad warm und bewölkt.",
            "In Ordnung, ich habe die Erinnerung zu Ihrem Kalender hinzugefügt.",
            "Beim Speichern des Dokuments ist ein Fehler aufgetreten.",
        ],
        "german",
    );
}

#[test]
fn portuguese_corpus_is_all_foreign() {
    assert_all(
        false,
        &[
            "Claro, posso ajudar você com isso.",
            "A compilação terminou com sucesso e todos os testes passaram.",
            "Sua reunião com a equipe de design começa em dez minutos.",
            "Encontrei três arquivos que correspondem à sua pesquisa.",
            "Você quer que eu leia o resumo em voz alta?",
            "Agora está fazendo quinze graus e o céu está nublado.",
            "Certo, adicionei o lembrete ao seu calendário.",
            "Houve um erro ao salvar o documento.",
        ],
        "portuguese",
    );
}

#[test]
fn italian_corpus_is_all_foreign() {
    assert_all(
        false,
        &[
            "Certo, posso aiutarti con questo.",
            "La compilazione è terminata con successo e tutti i test sono passati.",
            "La tua riunione con il team di design inizia tra dieci minuti.",
            "Ho trovato tre file che corrispondono alla tua ricerca.",
            "Vuoi che legga il riassunto ad alta voce?",
            "Adesso ci sono quindici gradi e il cielo è nuvoloso.",
            "Va bene, ho aggiunto il promemoria al tuo calendario.",
            "Si è verificato un errore durante il salvataggio del documento.",
        ],
        "italian",
    );
}

#[test]
fn short_foreign_phrases_with_known_words_fall_back() {
    for text in [
        "Hola amigo",
        "Bonjour le monde",
        "Gracias",
        "Hallo",
        "Das ist ein Test",
        "Muchas gracias por todo",
        "Obrigado, ate amanha",
    ] {
        assert!(!is_probably_english(text), "{text:?} must fall back");
    }
}

#[test]
fn short_english_and_technical_text_stays_english() {
    for text in [
        "Okay",
        "Sure.",
        "kubectl rollout status",
        "Compile Rust crate release build artifacts",
        "The cafe on the corner serves good coffee",
        "Hey Siobhan",
    ] {
        assert!(is_probably_english(text), "{text:?} must stay on Phonon");
    }
}

#[test]
fn mixed_text_is_decided_by_english_versus_foreign_evidence() {
    assert!(is_probably_english("Hola, how are you today?"));
    // One accented loanword is one point of foreign evidence; English evidence outweighs it.
    assert!(is_probably_english("Let's meet at the café tomorrow"));
}

#[test]
fn non_latin_scripts_and_empty_are_never_english() {
    for text in [
        "你好，世界",
        "مرحبا بالعالم",
        "Привет, мир",
        "",
        "   ",
        "...!?",
        "12345",
    ] {
        assert!(!is_probably_english(text), "{text:?} must fall back");
    }
}

#[test]
fn documented_false_positive_is_asserted_so_the_contract_stays_honest() {
    // Latin-script text that matches none of the foreign lists is spoken with English
    // pronunciation. This is a KNOWN limitation of the best-effort detector.
    assert!(is_probably_english("Zdravo prijatelju"));
}
