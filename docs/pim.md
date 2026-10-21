# Calendriers et contacts locaux

## Activer les vues

`0` revient à la vue mail complète. `1` ouvre les calendriers, `2` les contacts.
Les raccourcis `:calendar` et `:contacts` sont également disponibles. Une vue
facultative est masquée tant que sa section n'est pas activée et configurée.
Les comptes mail individuels restent accessibles via `:ns` / `:ns work`.

Exemple à ajouter au **config.toml de MailTUI** :

```toml
[calendar]
enabled = true

[[calendar.collections]]
id = "personal"
label = "Personnel"
path = "/home/alice/.local/share/calendars/personal"

[[calendar.collections]]
id = "work"
label = "Travail"
path = "/home/alice/.local/share/calendars/work"

[contacts]
enabled = true

[[contacts.books]]
id = "friends"
label = "Amis"
path = "/home/alice/.local/share/contacts/friends"

[[contacts.books]]
id = "team"
label = "Équipe"
path = "/home/alice/.local/share/contacts/team"
```

Chaque chemin absolu désigne **une collection vdir**, pas le répertoire parent
de toutes les collections. Les IDs doivent être uniques dans leur section.
Il n'y a pas d'expansion `~`. Les sections omises sont désactivées par défaut.
Une section activée sans collection est une erreur de configuration.

## Compatibilité khal et synchronisation

MailTUI utilise les mêmes fichiers locaux que khal : un fichier `.ics` par
événement/série dans chaque calendrier. Les carnets utilisent des fichiers
`.vcf` vCard 3.0 ou 4.0, compatibles notamment avec khard.

Dans la configuration **INI de khal**, le chemin de la collection correspond :

```ini
[calendars]
[[personal]]
path = /home/alice/.local/share/calendars/personal
[[work]]
path = /home/alice/.local/share/calendars/work
```

La synchronisation CalDAV/CardDAV relève de **vdirsyncer** ou de votre outil
existant ; khal est le client calendrier. MailTUI lit et écrit les collections
locales, sans lancer ces outils ni stocker leurs identifiants. Les écritures
seront envoyées à la prochaine synchronisation de cet outil.

## Parcourir et modifier

Les calendriers apparaissent dans une liste chronologique, avec calendrier,
lieu et heures selon la largeur du terminal. Les contacts sont triés par nom,
avec carnet, email, téléphone et organisation. Les collections sont réunies
dans chaque vue ; `/` filtre aussi sur leur ID ou leur libellé.

| Touche | Action |
|---|---|
| ↑ / ↓, PgUp / PgDn | Sélectionner une entrée |
| Entrée | Afficher les détails ; ↑/↓ pour les faire défiler |
| `c` ou `n` | Créer un événement ou un contact |
| `e` | Modifier l'entrée courante |
| `/` | Filtrer les entrées et les collections |
| Échap | Fermer le formulaire/détail, effacer le filtre, revenir |
| Ctrl+D | Supprimer l'entrée courante, avec confirmation |
| `:reload` | Relire les fichiers immédiatement |

Dans un nouveau formulaire, **←/→ dans le premier champ** choisit le calendrier
ou carnet de destination. Tab/Shift+Tab change de champ. **Ctrl+S** enregistre ;
Entrée passe au champ suivant puis enregistre depuis le dernier. Le carnet ou
calendrier d'une entrée existante reste fixe. Les formulaires défilent pour
garder le champ actif visible sur les petits terminaux.

Événement : titre, début, fin facultative, lieu, description, règle RRULE.
Les dates utilisent `YYYYMMDD` pour la journée entière, ou
`YYYYMMDDTHHMMSS` pour une heure locale flottante ; ajouter `Z` pour UTC.
Exemples : `20261201`, `20261201T093000`, `20261201T083000Z`.
La fin doit suivre le début et utiliser le même format. La fin d'un événement
sur journées entières est **exclusive** : du 1er au 3 décembre signifie les
1er et 2 décembre. Les TZID existants restent attachés aux événements importés.

Une règle comme `FREQ=WEEKLY;BYDAY=MO,WE` ou `FREQ=YEARLY;COUNT=3` définit une
série. La liste affiche **l'entrée maître de chaque série**, signalée par `↻`,
et non chaque occurrence développée dans un agenda journalier. Modifier ou
supprimer cette entrée agit sur la série entière. Les exceptions existantes
restent dans le même fichier ; elles ne disposent pas d'un éditeur individuel.

Contact : nom affiché, email principal, téléphone principal, organisation.
Les adresses/téléphones secondaires, photos, noms structurés, notes et autres
champs importés sont conservés lorsque les champs principaux sont modifiés.

## Fichiers et mises à jour

L'assistant LLM dispose des mêmes collections lorsque ces vues sont activées :
liste des calendriers/carnets, recherche, lecture et propositions de création,
modification ou suppression. Les outils sont disponibles en API comme en Codex.
`/apply N` applique une proposition via le worker PIM et affiche son résultat
dans la conversation. Voir [assistant.md](assistant.md#calendars-and-contacts).

- Les collections sont relues en arrière-plan toutes les cinq secondes, et à
  l'ouverture d'une vue. Un formulaire ouvert diffère cette relecture.
- Les opérations fichier passent par le worker, sans bloquer la boucle terminal.
- Les modifications conservent le **nom original du fichier**, indépendamment
  de son UID. Les nouveaux fichiers ont un nom unique et un UID généré.
- Les propriétés inconnues, paramètres de propriétés, alarmes, VTIMEZONE et
  exceptions de récurrence sont préservés. Les lignes sont repliées à 75 octets
  sans couper UTF-8, et écrites avec CRLF.
- Une modification utilise un fichier temporaire dans la même collection puis
  un renommage atomique. Avant de remplacer/supprimer, le contenu est comparé
  à la version chargée : si la synchronisation l'a changé, l'opération échoue
  et demande une relecture. Cela ne constitue pas un verrou partagé avec le
  synchroniseur.
- Un fichier invalide est signalé et ignoré ; les autres collections restent
  consultables. Les fichiers temporaires et les autres extensions sont ignorés.
- Le mode `--demo` ne simule que le mail : si des collections PIM sont activées,
  les opérations portent bien sur les fichiers configurés.

Références : [format vdir](https://vdirsyncer.pimutils.org/en/stable/vdir.html),
[configuration khal](https://khal.readthedocs.io/en/latest/configure.html).
