# Calendriers, contacts et tâches locaux

## Activer les vues

`0` revient à la vue mail complète. `1` ouvre les calendriers, `2` les contacts,
`3` les tâches. Les raccourcis `:calendar`, `:contacts` et `:tasks` sont également disponibles. Une vue
Calendrier ou Tâches est masquée tant que sa section n'est pas activée et configurée.
Contacts est toujours accessible : les contacts des mails proviennent de l'index SQLite.
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
Une section Calendrier/Tâches activée sans collection est une erreur de configuration.
La section Contacts peut rester sans carnet : les correspondants des mails restent disponibles.

### Contacts Mail et CardDAV

La vue `2` / `:contacts` contient deux branches, repliées par défaut :

- **Mail · indexed correspondents** : expéditeurs et destinataires des en-têtes
  présents dans l'index SQLite de MailTUI, tous comptes confondus. Les adresses sont
  dédupliquées sans distinction de casse, avec le nom disponible le plus récent.
  Le corps des messages n'est pas utilisé pour extraire des adresses. Ces fiches
  sont en lecture seule ; elles suivent l'ajout et la suppression de mails dans l'index.
- **CardDAV · address books** : fiches vCard des carnets locaux configurés.
  Elles restent indépendantes des correspondants Mail, même pour une adresse identique.
  `c`, `e`, Ctrl+D et Ctrl+S permettent de gérer ces fiches.

Entrée replie/déplie une catégorie ou ouvre une fiche. `/` filtre les noms,
adresses, téléphones, organisations, carnets et sources (`Mail` / `CardDAV`).

L'ajout et l'édition CardDAV utilisent un popup à champs encadrés, comme les tâches :
**Name**, **Email**, **Phone**, **Organization**, et boutons **Save / Cancel**.
**PgUp / PgDn** change de section : **General**, **Personal**, **Work**, **Other**.
Chaque section d'adresse comporte **Street / address lines**, **City**,
**Region / State**, **Postal code**, **Country**. La rue accepte plusieurs lignes
par collage ou **Alt+Entrée**. Tab parcourt les champs et boutons de la section courante.
**Tab / Shift+Tab** change le champ actif ; **Entrée** avance ou active le bouton
sélectionné ; **Ctrl+S** enregistre directement et **Échap** annule.
À la création, Entrée sur **Address book** ouvre le menu des carnets, à sélectionner
avec ↑/↓ puis Entrée ; ←/→ permet aussi de changer de carnet. Le carnet est fixe
pendant l'édition d'une fiche existante. Le champ actif reste visible sur les petits terminaux.

Les adresses sont stockées sous forme de propriétés vCard structurées `ADR`, avec
`TYPE=HOME`, `TYPE=WORK` et `TYPE=OTHER`, correspondant aux adresses personnelle,
professionnelle et autre de Zimbra. Les adresses sans type sont lues comme « Other ».
Le formulaire modifie la première adresse de chaque type ; les adresses secondaires,
boîtes postales, compléments structurés et paramètres importés sont conservés.
Une adresse importée non modifiée conserve sa propriété d'origine. Les détails
affichent les adresses renseignées et `/` filtre aussi leurs composantes.
Les modifications d'adresse suivent la même sauvegarde locale et synchronisation
CardDAV automatique que les autres champs.

Pour découvrir les carnets sous un répertoire parent au démarrage :

```toml
[contacts]
enabled = true
path = "/home/alice/.local/share/contacts"
sync_command = ["vdirsyncer", "sync", "my_contacts"]
sync_timeout_seconds = 120
```

Chaque sous-répertoire direct non caché devient un carnet. Les déclarations
`[[contacts.books]]` restent utilisables pour choisir les IDs et libellés de carnets
précis ; elles ont priorité sur la découverte pour un même chemin.

La synchronisation des contacts utilise **CardDAV** (CalDAV est destiné aux
calendriers et tâches). Exemple dans `~/.config/vdirsyncer/config`, à adapter :

```ini
[general]
status_path = "~/.local/state/vdirsyncer/status/"

[pair my_contacts]
a = "contacts_local"
b = "contacts_remote"
collections = ["from a", "from b"]

[storage contacts_local]
type = "filesystem"
path = "~/.local/share/contacts/"
fileext = ".vcf"

[storage contacts_remote]
type = "carddav"
url = "https://dav.example.com/addressbooks/"
username = "alice"
password = "YOUR_APP_PASSWORD"
```

Initialiser avec `vdirsyncer discover my_contacts`, puis `vdirsyncer sync my_contacts`
avant de démarrer MailTUI pour que les carnets soient découverts.
Dans Contacts, **`:sync` ou Ctrl+R** exécute ensuite `contacts.sync_command`.
Une création, modification ou suppression réussie déclenche aussi cette commande.
La commande s'exécute en arrière-plan, sans shell ; les demandes simultanées
sont regroupées par ressource, indépendamment des synchronisations Calendar et Tasks.
Après la commande, les fichiers sont rechargés pour afficher les contacts téléchargés.
Un échec ou dépassement du délai est signalé, en conservant les modifications locales.
Sans commande configurée, les carnets restent utilisables localement.

### Découvrir tous les calendriers d'un répertoire

Pour charger les calendriers sans déclarer chaque collection, utilisez la section
**`[calendars]` au pluriel** :

```toml
[calendars]
enabled = true
path = "/home/alice/.local/share/calendars"
```

Au démarrage, chaque sous-répertoire direct non caché devient un calendrier :
`calendars/personal/` et `calendars/work/`, par exemple. Le nom du répertoire
sert d'ID et de libellé ; les collections découvertes sont triées par nom.
Les fichiers du répertoire parent sont ignorés et la découverte n'est pas récursive.
Un calendrier vide est accepté, ce qui permet d'y créer son premier événement.
Le chemin doit être absolu et lisible ; un parent vide sans collection explicite
est une erreur de configuration.

Cette section active la vue Calendrier. Elle peut être combinée avec
`[[calendar.collections]]` : une collection explicite avec le même chemin garde
son ID et son libellé et n'est pas ajoutée une seconde fois. Les IDs doivent
rester uniques entre toutes les collections.

Les événements des collections découvertes bénéficient des mêmes fonctions
de lecture, création, modification et de l'assistant. Après ajout ou suppression
d'un **répertoire de calendrier**, redémarrez MailTUI pour actualiser la liste des
collections ; les fichiers d'événements sont toujours relus automatiquement.

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
locales et peut lancer votre commande de synchronisation pour les calendriers
et les tâches. Les identifiants distants restent gérés par cet outil.

### Commandes de synchronisation

```toml
[calendars]
enabled = true
path = "/home/alice/.local/share/calendars"
sync_command = ["vdirsyncer", "sync", "my_calendars"]
sync_timeout_seconds = 120

[tasks]
enabled = true
path = "/home/alice/.local/share/tasks"
sync_command = ["vdirsyncer", "sync", "my_tasks"]
sync_timeout_seconds = 120
```

- **`Ctrl+R`** ou **`:sync`** lance la commande de la vue courante (Calendrier, Contacts ou Tâches).
  Le raccourci `Ctrl+R` est affiché dans le bandeau supérieur, y compris en mode compact.
- Une création, modification ou suppression réussie déclenche aussi la commande
  de la ressource modifiée, même si l'on a changé de vue entre-temps. Les
  modifications de calendrier appliquées depuis l'assistant suivent le même chemin.
- L'exécution est en arrière-plan, sans bloquer la navigation. Plusieurs demandes
  pour une même ressource pendant une synchronisation sont regroupées en une
  exécution supplémentaire. Calendriers, contacts et tâches ont chacun leur propre commande.
- Le programme et ses arguments sont une liste de chaînes, **sans shell**, sans
  argument ajouté implicitement. La commande doit fonctionner sans saisie interactive.
  Sa sortie n'est pas affichée dans le terminal ; le succès ou l'échec est signalé
  dans la barre de statut. Le délai maximal est de 120 secondes par défaut.
- Une fois la commande terminée, les fichiers locaux sont relus ; un formulaire
  ouvert diffère cette relecture. Un échec de synchronisation ne supprime pas les
  modifications locales. Une commande dépassant son délai est arrêtée.
- Sans `sync_command` (ou avec `[]`), aucune commande n'est exécutée. Il n'y a
  pas de synchronisation distante au démarrage ni périodique.

Pour conserver uniquement vos collections explicites `[[calendar.collections]]`,
ajoutez une section `[calendars]` avec `sync_command`, en laissant `enabled = false`
(valeur par défaut) : la découverte du répertoire reste désactivée et la commande
s'applique aux calendriers explicites activés dans `[calendar]`.

### Listes de tâches VTODO

`[tasks].path` désigne un répertoire parent, avec une liste par sous-répertoire
direct non caché. Le nom du répertoire sert d'ID et de libellé. Les listes vides
sont acceptées ; un parent sans aucune liste est une erreur de configuration.
Comme pour les calendriers, les nouvelles listes sont découvertes au démarrage.

Chaque fichier `.ics` contient une tâche/série **VTODO**. Les collections CalDAV
mixtes sont acceptées : la vue Calendrier ignore les fichiers ne contenant que des
VTODO, et la vue Tâches ignore ceux ne contenant que des VEVENT. Les mêmes chemins
peuvent donc être configurés dans les deux sections.

Les tâches sont triées avec les tâches clôturées (terminées ou annulées) en dernier, puis par échéance et
titre ; les tâches sans échéance suivent celles avec une échéance. `/` filtre
sur le titre, la description, le statut ou la liste. Le formulaire propose :

- Titre obligatoire, description, début et échéance facultatifs.
- Dates `YYYYMMDD` ou `YYYYMMDDTHHMMSS[Z]`. Si début et échéance sont renseignés,
  l'échéance doit suivre le début et utiliser le même format.
- Statut `NEEDS-ACTION`, `IN-PROCESS`, `COMPLETED` ou `CANCELLED`.
- Priorité de `0` (non spécifiée) à `9` (la plus faible), `1` étant la plus haute.

### Popup d'édition d'une tâche

`c` crée une tâche ; **Entrée ou `e` sur une tâche** ouvre le même popup d'édition,
centré, avec de vrais contrôles :

- **Task list** : menu de destination à la création ; la liste d'une tâche
  existante reste fixe.
- **Title** et **Description** : saisie de texte, avec description multiligne.
- **Status** : menu To do / In progress / Completed / Cancelled.
- **Priority** : menu de 0 (None) à 9, avec les couleurs de priorité.
- **Due date** et **Start date** : sélecteurs de date facultatifs.
- **Save / Cancel** : boutons explicites ; Ctrl+S enregistre aussi.

Tab/Shift+Tab déplace le focus. Entrée ou Espace ouvre un menu, ↑/↓ choisit
et Entrée applique ; Échap revient au formulaire sans appliquer le choix.
Les chiffres sélectionnent directement une option. Depuis le formulaire,
←/→ permet également de parcourir les choix. Dans la description, Entrée
ajoute une ligne ; Tab passe aux boutons. Échap ferme le formulaire sans écrire.

Le sélecteur de date affiche un calendrier mensuel : ←/→ change de jour,
↑/↓ de semaine, PgUp/PgDn de mois, Home revient à aujourd'hui. Tab passe aux
options : **Date only**, **With time** ou **With time · UTC**, puis aux heures
et minutes ajustables avec les flèches. Entrée applique la date sélectionnée ;
Suppr/Backspace ou le bouton **No date** retire la date. Les petites tailles
de terminal utilisent un sélecteur compact et gardent le champ actif visible.

Les dates restent stockées au format ICS ; le formulaire ne demande plus de
les saisir sous cette forme. Les écritures passent par le worker et déclenchent
la synchronisation configurée. Les propriétés inconnues, le fichier original,
les fuseaux et alarmes importés sont préservés.

Le statut `NEEDS-ACTION` est affiché **« To do »** dans la liste, les détails et
le formulaire. Le formulaire accepte aussi cette valeur ; le fichier ICS conserve
le statut standard `NEEDS-ACTION`. La recherche reconnaît les deux libellés.
Une échéance sans heure est affichée sous la forme `AAAA-MM-JJ`, sans « all day ».

La couleur des lignes indique la priorité : **rouge pour 1–4** (haute),
**jaune pour 5** (moyenne), **vert pour 6–9** (basse). Une priorité vide ou 0
garde le style normal ; les tâches clôturées gardent leur style atténué.
Le thème sans couleurs distingue les priorités par gras, italique et atténuation.

Passer à `COMPLETED` renseigne aussi la date de fin et `PERCENT-COMPLETE:100`.
Rouvrir une tâche retire sa date de fin ; `NEEDS-ACTION` remet la progression à 0.
Les propriétés inconnues, alarmes, fuseaux et règles de récurrence importées sont
conservés. La vue montre les maîtres de séries, sans développer les occurrences.
Modifier ou supprimer une tâche récurrente agit sur sa série entière.

## Parcourir et modifier

Les vues Calendrier et Tâches utilisent un arbre : **calendrier/liste → entrées**,
comme les dossiers et mails de la vue Messages. Les branches sont repliées par
défaut ; Entrée sur une branche la replie/déplie. Sur une tâche, Entrée ouvre
le popup d'édition ; sur un événement ou un contact, elle ouvre les détails.
Plusieurs branches peuvent rester dépliées. Les calendriers sans
événement visible sont masqués, même si leur nom correspond au filtre. Un calendrier
contenant seulement des événements passés réapparaît avec `v` ; un calendrier
réellement vide reste masqué. Les événements sont classés par leur **date de début
affichée**, en tenant compte du fuseau horaire, au sein de chaque calendrier.
Les tâches restent classées par statut/échéance au sein de leur liste.

Les événements **en cours ou futurs** sont affichés par défaut. Les événements
terminés sont masqués en tenant compte de leur fin, du caractère exclusif des
fins de journées entières et du fuseau horaire ; les dates flottantes utilisent
le fuseau local. Les séries avec des occurrences à venir restent visibles,
même si leur date de début maître est passée. Ce calcul s'effectue dans le worker.
Une horloge actualise le surlignage et recalcule les occurrences lorsqu'une fin
d'événement est dépassée, à partir des documents en mémoire, sans relecture des fichiers.

Les événements actuellement en cours sont **surlignés en vert, en gras et
soulignés**, à partir des dates de l'occurrence affichée. Le thème sans couleurs
conserve le gras et le soulignement. La colonne Location est retirée de la liste ;
le lieu reste consultable dans les détails et modifiable dans le formulaire.

Les tâches `COMPLETED` et `CANCELLED` sont masquées par défaut, y compris immédiatement
après une édition. **`v`** affiche ou masque les événements passés dans Calendrier,
et les tâches clôturées (terminées ou annulées) dans Tâches. Les deux
réglages sont indépendants et conservés lorsqu'on change de vue. Le bandeau
supérieur affiche `Show past`/`Hide past` ou `Show closed`/`Hide closed`.

Les contacts restent triés par nom, avec carnet, email, téléphone et organisation.
`/` filtre aussi sur l'ID ou le libellé des collections et révèle les entrées
correspondantes sans modifier les états de repli mémorisés. Une création depuis
une branche choisit cette collection par défaut ; les branches ne sont pas
traitées comme des événements/tâches par `e` ou Ctrl+D. Les détails d'événement
ne montrent pas le chemin du fichier `.ics`.

| Touche | Action |
|---|---|
| ↑ / ↓, PgUp / PgDn | Sélectionner une entrée |
| Entrée | Replier/déplier une branche ; éditer une tâche ou afficher les détails d'un événement/contact |
| `c` ou `n` | Créer un événement ou un contact |
| `e` | Modifier l'entrée courante |
| `/` | Filtrer les entrées et les collections |
| Échap | Fermer le formulaire/détail, effacer le filtre, revenir |
| Ctrl+D | Supprimer l'entrée courante, avec confirmation |
| `:reload` | Relire les fichiers immédiatement |
| `Ctrl+R` ou `:sync` | Synchroniser les calendriers ou les tâches de la vue courante |
| `v` | Afficher/masquer les événements passés ou les tâches clôturées |

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
série. La liste affiche **une seule ligne par série**, signalée par `↻`, avec les
dates de début et de fin de sa **prochaine occurrence en cours ou à venir**.
Les exclusions `EXDATE`, dates supplémentaires `RDATE`, exceptions déplacées ou
annulées et changements de fuseau horaire sont pris en compte. La durée de
l'événement est conservée, y compris les fins exclusives des journées entières.
Les colonnes de début et de fin sont affichées ensemble à partir de 75 colonnes.

La règle RRULE et les dates du maître restent dans le détail/formulaire, sans
encombrer la liste principale. En mode historique, une série terminée affiche
les dates de sa dernière occurrence. Cette projection ne modifie pas le fichier
ICS : modifier ou supprimer la ligne agit toujours sur la série entière. Les
exceptions existantes restent dans le même fichier ; elles ne disposent pas
d'un éditeur individuel.

Contact : nom affiché, email principal, téléphone principal, organisation.
Les adresses/téléphones secondaires, photos, noms structurés, notes et autres
champs importés sont conservés lorsque les champs principaux sont modifiés.

## Fichiers et mises à jour

L'assistant LLM dispose des mêmes collections lorsque ces vues sont activées :
liste des calendriers/carnets, recherche, lecture et propositions de création,
modification ou suppression. Les outils sont disponibles en API comme en Codex.
`/apply N` applique une proposition via le worker PIM et affiche son résultat
dans la conversation. Voir [assistant.md](assistant.md#calendars-and-contacts).

- Les fichiers sont surveillés via `notify`, avec **inotify sous Linux**. Une
  création, modification, suppression ou un renommage déclenche une relecture
  en arrière-plan, sans polling périodique des répertoires. Les rafales sont
  regroupées pendant environ 200 ms, notamment lors des sauvegardes atomiques.
- Les répertoires de collection et leurs parents sont surveillés : les fichiers
  remplacés par renommage et les collections supprimées/recréées restent suivis.
  Un chemin configuré encore absent est couvert par son ancêtre existant.
  Les simples accès en lecture ne déclenchent pas de nouvelle relecture.
- Les formulaires diffèrent les mises à jour sans perdre la notification. Une
  relecture reste disponible à l'ouverture d'une vue, avec `:reload`, après une
  écriture ou à la fin d'une synchronisation. La découverte de nouvelles listes
  non configurées reste effectuée au démarrage.
- Le contrôle du temps, une fois par seconde, n'accède pas aux fichiers : il
  permet le surlignage des événements en cours et demande au worker un recalcul
  des dates depuis les documents chargés seulement lorsqu'une échéance est passée.
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
