# MailTUI

Client email TUI **local-first**, en Rust, inspiré de k9s, avec calendriers et
contacts locaux facultatifs. Les touches numériques sélectionnent les vues :
**`0` Mail**, **`1` Calendrier**, **`2` Contacts**. Les deux dernières ne sont
disponibles que si elles sont activées et configurées dans le TOML.

La navigation mail utilise **une seule table `Messages`** et un seul curseur :

```text
▸ work
▸ personal
```

Dans le scope **`all` (`0`)**, les comptes sont les branches de premier
niveau. Pour un seul compte (`:ns work`), l'arbre commence directement par ses
dossiers. Les mails sont des lignes enfants de leur dossier, **dans cette
même table**. Plusieurs dossiers peuvent rester dépliés simultanément.

Les comptes et dossiers sont **repliés par défaut**. Entrée sur un compte
affiche ses dossiers ; Entrée sur un dossier affiche ses mails. Les opérations réelles sont mises
en file avec l'API publique `PimdirProducer`, puis appliquées par Neverest.
Le mode démonstration modifie seulement la mémoire et n'envoie aucun message.

## Démarrer

Prérequis : Rust stable ≥ 1.89, compilateur C et linker. SQLite est compilé
avec l'application. Les dépendances sont fixées par `Cargo.lock`, io-pimdir
par une révision Git vérifiée.

```sh
cargo run --locked -- --demo
```

Sans terminal interactif :

```sh
cargo run --locked -- --demo --diagnostics
cargo run --locked -- --demo --snapshot 120x40
cargo run --locked -- --keybindings
```

Les captures exactes sont dans [`tests/snapshots/`](tests/snapshots/), aux
tailles 40×12, 80×24, 120×40 et 180×50. Toutes affichent le même arbre pleine
largeur : aucun volet latéral ni basculement entre listes dossiers/messages.
Fond noir, bandeau cyan, **raccourcis sur deux colonnes** en haut, sélecteur de
vues et indicateur des mails en cours d'envoi à côté, logo sur grand terminal.
Les anciens champs Context/Account/Directory/Version ont été retirés du bandeau.

## Parcourir l'arbre Messages

| Touche | Action |
|---|---|
| ↑ / ↓ | Parcourir toutes les lignes : comptes, dossiers, mails |
| Entrée | Replier/déplier un compte ou dossier ; lire un mail |
| `h` / `H` | HTML sans / avec images distantes |
| PageDown / PageUp | Défilement par page |
| `/` | Recherche locale et sélecteur de résultats |

Au lancement, tous les comptes et dossiers sont repliés, y compris les inbox. Aucun mail
n'est chargé avant leur ouverture. Déplier un dossier charge sa première page de métadonnées
locales en arrière-plan. La ligne **Load more**, ou `:more` avec le dossier
ciblé, charge sa page suivante. Lire les corps MIME demande une ouverture
explicite du message : le dépliage ne les charge pas.

Replier conserve le cache et les marques des messages. Le redépliage d'un
dossier déjà chargé est immédiat. Les comptes et dossiers ne sont jamais
sélectionnés comme des emails par les actions de marquage/suppression.
Les compteurs **NEW/TOTAL** affichent les non-lus et le total du store, pas
seulement la page chargée.

Les alias `:folders`, `:directories`, `:dirs` et `:dir` restent dans
**Messages** : ils ciblent le dossier de la ligne courante. Avec un compte
actif, `:dir "Projects/Rust/MailTUI"` déplie les ancêtres et ce dossier,
charge ses mails puis le cible sans fermer les autres dossiers.

### Comptes comme namespaces k9s

- **`0`** : tous les comptes, matérialisés par les branches de l'arbre.
- Les touches **`1` et `2`** sélectionnent désormais le calendrier et les contacts ;
  les chiffres `3`…`9` ne sélectionnent plus de compte.
- **`:ns`** ou `:accounts` : sélecteur comprenant la ligne `all`.
- **`:ns work`**, **`:ns all`** : activation directe.

Replier un compte dans le scope `all` ne change pas le namespace. Changer
de compte conserve ses états de repli/dépliage et efface les marques. Échap
depuis un lecteur ou une aide conserve le compte actif et restaure le contexte.
`:inbox` déplie les inbox du compte actif ; `:unified` fait de même dans `all`.
Le sélecteur de comptes, le lecteur de message et les diagnostics sont des
outils secondaires ; la navigation des dossiers/mails reste toujours dans
la vue Messages.

### Filtrer, chercher et agir sur les mails

- **`/`** ouvre le sélecteur de recherche locale : taper des mots, utiliser
  ↑/↓ pour choisir un résultat et Entrée pour lire le message. Échap revient
  à l'affichage précédent. Les dossiers peuvent rester repliés : la recherche
  utilise l'index complet, pas seulement les pages affichées.
- **`:search <query>`** utilise le même index local et range les résultats
  dans l'arbre Messages. Le compte actif borne la recherche ; `0` permet tous
  les comptes. Cette recherche n'exécute aucune commande de recherche IMAP.
- La date affiche **`AAAA-MM-JJ HH:MM`**, sans secondes. La colonne ST est
  supprimée ; les mails marqués changent de couleur sur toute leur ligne.
- **Espace** marque/démarque un mail ; **a** marque les mails visibles.
  Les lignes de compte/dossier ne sont pas sélectionnées comme des emails.
  Replier un dossier conserve ses marques.
- **Ctrl+D** agit sur les mails marqués, sinon le mail courant. Le popup
  démarre sur **Cancel** : Tab puis Entrée sélectionne **Delete** et déplace
  vers Trash. Une ligne compte/dossier seule ne supprime rien.
- **i/u**, **y** : lu/non lu et copie (file pimdir pour les comptes réels).
- **m** ouvre le popup de déplacement : taper un filtre de type fzf
  (sous-séquence, par exemple `prrmt`), choisir avec ↑/↓ ou Tab, puis Entrée.
  Les destinations appartiennent au compte des mails sélectionnés.
- **n** ouvre la création de dossier. Tab choisit la racine du compte ou le
  dossier courant comme parent ; Entrée crée/met en file la création.
- L'affichage et l'index suivent automatiquement les changements locaux de pimdir.
- **Shift+N/A/S** trie les mails par sujet/date/statut **dans chaque dossier** ;
  **Shift+P** trie les branches de comptes ; **s** fait tourner le tri des mails.
- **?** affiche l'aide issue du registre réel ; **:** ouvre les commandes.

Tous les raccourcis : [`docs/keybindings.md`](docs/keybindings.md).
Les thèmes `k9s-dark`, `high-contrast`, `no-color` se changent avec `:theme`.
`NO_COLOR` active automatiquement le mode sans couleurs. `:q` quitte.
Pour Ctrl+C, une première pression affiche une indication en bas ; une
deuxième **dans les trois secondes** quitte. Après expiration, une nouvelle
pression ouvre une nouvelle fenêtre de confirmation.

## Calendriers et contacts

Les vues **`1` Calendrier** et **`2` Contacts** utilisent des collections locales
`.ics` et `.vcf`, compatibles avec les répertoires khal/khard et vdirsyncer.
Elles sont désactivées par défaut. Plusieurs calendriers et carnets peuvent
être configurés ; `c` crée, `e` modifie, Ctrl+D supprime, `/` filtre, et
Ctrl+S enregistre dans le formulaire. Les fichiers sont relus automatiquement.

Configuration TOML et détails des formats : **[docs/pim.md](docs/pim.md)**.

## Composer un email

**c** (ou `:compose`) ouvre le compositeur intégré : compte expéditeur,
destinataires, sujet et corps multiligne. Les destinataires sont des adresses
séparées par des virgules. Tab/Shift+Tab change de champ ; ↑/↓ sur From choisit
le compte ; Entrée insère une ligne dans le corps. Unicode, collage et édition
au clavier sont pris en charge (corps jusqu'à 64 Kio).

- **Ctrl+S** met le message MIME en file pour soumission par Neverest.
- **Ctrl+O** enregistre un brouillon (dossier avec rôle `drafts` pour un vrai compte).
- **Ctrl+T** ouvre le navigateur de pièces jointes (sélection multiple).
- **Échap** ferme le compositeur.

### Pièces jointes à envoyer

Dans le navigateur, taper un filtre fuzzy pour retrouver un fichier ou un
dossier. ↑/↓ déplace le curseur ; Entrée ouvre un dossier ou ajoute le fichier
courant aux pièces jointes et revient au compositeur. **Tab** marque/démarque
plusieurs fichiers avant validation. Avec un filtre vide,
Backspace remonte au parent. Les marques sont conservées entre dossiers.

**Entrée sur un fichier** ou **Ctrl+S** valide la sélection et revient au compositeur, où les noms des
fichiers attachés sont affichés. **Ctrl+U** efface les marques du navigateur ;
Échap revient sans modifier les pièces jointes. Ctrl+T permet de rouvrir la
sélection pour ajouter ou retirer des fichiers.

Les fichiers sont lus hors de la boucle de rendu et intégrés au MIME, avec
leurs noms et types de contenu, pour l'envoi comme pour les brouillons.
La taille totale du message MIME encodé est limitée à 16 Mio. Si un fichier
est devenu illisible ou manque, le compositeur et sa sélection sont conservés
pour correction ; aucun message incomplet n'est mis en file.

Le MIME est construit avec mail-builder. Le blob et l'intention `submit`
portant l'enveloppe sont persistés dans pimdir. Neverest utilise le transport
SMTP/Graph configuré et conserve les résultats/erreurs de soumission dans sa
file et ses rapports. Une mise en file n'est pas annoncée comme une livraison.
Le compositeur reste ouvert avec son contenu si la mise en file échoue.

### Enregistrer les pièces jointes reçues

Dans un message ouvert, **Ctrl+S** affiche ses pièces jointes avec leur icône,
nom et taille. Le trombone **📎** remplace `+` dans la liste des mails ; les
dossiers ouverts/fermés utilisent **📂 / 📁**, et les fichiers ont une icône de type.

- Taper un filtre fuzzy et choisir avec ↑/↓.
- **Entrée sans marque** choisit uniquement le fichier pointé.
- **Espace / Tab** marque plusieurs fichiers ; **Ctrl+A** sélectionne/désélectionne
  tous les résultats du filtre. **Entrée** ouvre le choix de destination.
- Modifier le dossier proposé puis **Entrée** pour enregistrer ; **Échap** revient
  à la sélection, ou ferme celle-ci. Le dossier est créé s'il n'existe pas.

La destination proposée respecte `XDG_DOWNLOAD_DIR` et `user-dirs.dirs`, avec
`~/Downloads` en repli. Les chemins `~/…` sont acceptés. Un fichier déjà présent
est conservé : les doublons reçoivent un suffixe, par exemple `rapport (1).pdf`.
Les fichiers sont décodés depuis le MIME local et écrits dans le worker ;
le message reste ouvert après l'enregistrement. Dans le compositeur, Ctrl+S
garde sa fonction d'envoi.

## Lire un pimdir existant

Adapter [`config.example.toml`](config.example.toml) dans
`~/.config/mailtui/config.toml`, ou indiquer un chemin :

```sh
cargo run --locked -- --config /chemin/config.toml --diagnostics
cargo run --locked -- --config /chemin/config.toml
```

`store` est le chemin absolu réel du répertoire contenant `pimdir.db` et
`objects/`. Les exemples de chemins ne sont pas une convention Neverest
garantie. Les IDs des dossiers conservent leurs namespaces upstream, par
exemple `source/INBOX`, et leurs parents proviennent de l'API publique.
L'ID de compte `all` est réservé. Un store compatible peut être lu hors ligne.
Les serveurs, droits et secrets sont configurés séparément chez Neverest.
Voir les [vérifications upstream](docs/upstream-compatibility.md).

Les anciens champs `ui.preview_pane` et `ui.directory_pane` sont acceptés
pour compatibilité, mais ignorés : ils ne créent plus de volets.

## Affichage dynamique avec Carillon et Neverest

**Carillon pilote la réception indépendamment de MailTUI.** MailTUI lit
pimdir et surveille ses curseurs publics de changement toutes les **deux secondes**,
y compris les écritures SQLite en WAL. Deux nouveaux messages déposés par le
service externe apparaissent dans les dossiers ouverts sans touche de refresh.
Les compteurs des comptes/dossiers repliés sont actualisés aussi ; ouvrir ensuite
une branche montre son contenu à jour. L'index de recherche suit ces changements
en arrière-plan, même si le popup de recherche est déjà ouvert.

Les comptes et dossiers conservent leur état déplié/replié et le focus est
restauré par identifiant quand le message reste dans la page chargée. Pendant
une lecture, un popup ou une édition, le refresh de la liste est différé jusqu'au
retour au parcours afin de préserver l'interaction. `:reload` reste une relecture
locale facultative.

Les suppressions, déplacements et changements de flags sont déposés dans
la file producteur publique pimdir, puis traités par le service externe.
Le `store` configuré
dans MailTUI doit désigner le même répertoire que `store.root` chez Neverest.
Les capacités déclarées par la source déterminent les actions permises.

### Envoi de mail et Pending

Après un envoi explicite, MailTUI lance **`neverest sync -a <compte> --intents-only`**
pour soumettre le message sans attendre Carillon. Le compte est toujours explicite :
`accounts[].neverest_account` s'il est renseigné, sinon l'ID du compte MailTUI.
Il est capturé depuis le compositeur, même si l'utilisateur change de compte
avant le retour du worker. Les envois du même compte ne se chevauchent pas.

**`Pending: 1 sending`** indique un mail non encore confirmé. Il disparaît du
compteur lorsque Neverest retire sa soumission après l'accusé SMTP ; une action
simplement déposée dans la file n'est pas une confirmation d'envoi. Un échec
reste visible (`failed`, détails dans `:outbox`). Les changements de flags,
déplacements et créations de dossiers ne contribuent pas à ce compteur.
La file d'envoi est relue toutes les deux secondes, même sans changement de mails
ou avec un formulaire ouvert. Les copies Sent sont répliquées par Carillon.

Configuration facultative pour l'exécutable ou un fichier Neverest particulier :
```toml
[delivery]
command = ["neverest"]
timeout_seconds = 240
# config = "/home/me/.config/neverest/config.toml"
```

## Recherche locale persistante — SQLite FTS5

MailTUI conserve sa propre base dans
**`$XDG_DATA_HOME/mailtui/search-<identifiant>.sqlite3`**, ou
`~/.local/share/mailtui/` si XDG_DATA_HOME n'est pas défini. Elle contient les
mots indexés et les pointeurs `(compte, dossier, seq)` vers les messages.
Les sources configurées distinguent les bases de plusieurs configurations.

La construction initiale se fait en arrière-plan. Ensuite, les curseurs de
changement publics de pimdir permettent des mises à jour incrémentales :
nouveaux messages, contenu/flags modifiés, déplacements, suppressions et
changements de dossiers. Les corps disponibles plus tard sont ajoutés aussi.
La base est réutilisée après redémarrage ; les recherches ne reconstruisent
pas tout le corpus et ne contactent aucun serveur.

Avec **`/`**, les mots recherchent les sujets, expéditeurs, destinataires et
corps disponibles localement ; les préfixes de mots sont acceptés. La recherche
fuzzy sur les métadonnées permet aussi une saisie de type `kbrnts` pour
« Kubernetes ». Les résultats sont actualisés au fil de la saisie, avec un
maximum de 200 résultats dans le popup. Les opérateurs suivants sont disponibles :

```text
:search kubernetes
:search "incident report"
:search from:alice@example.com subject:release
:search account:work folder:INBOX is:unread
:search has:attachment after:2026-09-01 before:2026-11-01
```

ET implicite ; dates calendaires UTC à bornes exclusives ; opérateurs
`from`, `to`, `subject`, `account`, `folder`, `is:read|unread|flagged`,
`has:attachment`, `after`, `before`. Le scope du compte est appliqué avant
la limite de 200 hits. Les IDs de compte/dossier sont exacts ; from/to/subject
sont des sous-chaînes. `OR`, `NOT`, parenthèses et négation sont refusés.

Les corps décodables dans la limite MIME de lecture de 16 Mio sont indexés ;
un corps absent reste recherchable par métadonnées jusqu'à sa réception.
Le mode démo utilise un index mémoire isolé. Aucun résultat de recherche n'est
demandé à IMAP : Neverest synchronise les données, MailTUI gère leur index local.

## Assistant LLM facultatif

**Ctrl+A** ou **`:ai`** ouvre une fenêtre de conversation centrée, à **80 % de
la largeur et de la hauteur** du terminal, devant le mail ou la liste qui restent
visibles en arrière-plan. Le mail ouvert ou pointé sert de contexte.
Depuis une branche/vue globale, le périmètre est celui
du compte actif, ou tous les comptes. **`:ai global`** commence une conversation
sans mail attaché ; **`:ai new`** recommence depuis le contexte courant.
Le mail et le compte sont attachés à la conversation : changer de vue ne les
modifie pas. Échap ferme la conversation ; Ctrl+A ou `:ai` la reprend.

L'assistant peut rechercher dans l'index local, lire des mails, retrouver les
fils via `Message-ID`/`In-Reply-To`/`References`, chercher des échanges connexes,
préparer une réponse et proposer un classement. Les références **[N]** renvoient
aux sources ; dans le champ de conversation :

- `/open N` ouvre le mail source ; `/sources` affiche leurs titres.
- `/apply N` ouvre une proposition de réponse dans le compositeur, ou applique
  un déplacement ou une opération calendrier/contact proposée. Une réponse conserve les en-têtes du fil ; l'envoi
  reste l'action explicite Ctrl+S du compositeur.
- `/stop` interrompt la requête ; PgUp/PgDn parcourt la conversation.
- `/models` (ou `:ai models`) affiche le catalogue de modèles exposé par l'instance
  Codex connectée, avec les identifiants exacts à utiliser dans `[llm].model`.
- `/new`, `/global`, `/status`, `/login`, `/logout` correspondent aux sous-commandes
  `:ai`. Ctrl+A garde son rôle d'édition/sélection dans les champs et sélecteurs.

La conversation est conservée **en mémoire pendant le lancement courant**.
Les recherches mail restent limitées aux données locales et au compte de la conversation.
Le modèle doit prendre en charge les appels d'outils. Une recherche retourne au
plus 200 candidats, par pages de 20 ; les corps et fils sont bornés avec une
indication de troncature. L'index peut encore être en cours de mise à jour.

Si les sections calendrier/contacts sont activées, l'assistant dispose aussi
d'outils de liste, recherche et lecture des événements et contacts, ainsi que
de propositions de création, modification et suppression. Il utilise les
collections configurées dans le TOML, indépendamment du compte mail actif.
Exemples : « retrouve le contact d'Ada », « crée un rendez-vous à partir de ce
mail dans mon calendrier travail ». **`/apply N`** applique la proposition via
le worker local ; le résultat apparaît ensuite dans la conversation.

La recherche mail du LLM interroge **la base SQLite de MailTUI**, sans recherche
IMAP ni reconstruction depuis le store en mode réel. Les modes `auto`, `literal`
et `structured` permettent au modèle de distinguer texte et filtres. Les sujets
avec parenthèses et les apostrophes sont pris en charge ; un corps MIME absent
n'empêche plus de chercher dans l'index. Une erreur de base ou de syntaxe est
retournée explicitement, avec une indication de correction.

### Endpoint compatible OpenAI et clé API

Dans `config.toml` :

```toml
[llm]
enabled = true
provider = "openai_compatible"
endpoint = "https://votre-endpoint.example/v1"
model = "nom-du-modele"
api = "chat_completions" # ou "responses"
timeout_seconds = 180
max_tool_rounds = 12

[llm.auth]
method = "api_key"
api_key_env = "MAILTUI_LLM_API_KEY"
```

La clé est lue dans cette variable d'environnement lors de la première question.
Le client utilise le streaming SSE et les appels d'outils du protocole choisi.
Un endpoint local peut utiliser une URL HTTP et une valeur de clé factice s'il
n'exige pas d'authentification. Les documents nécessaires à la question sont
transmis à l'endpoint configuré.

### Codex OAuth

```toml
[llm]
enabled = true
provider = "codex"
model = "nom-du-modele-codex"
codex_command = ["codex"]
# codex_home = "/chemin/absolu/vers/un/home/codex/dedie"

[llm.auth]
method = "codex_oauth"
```

Installer une version récente de Codex prenant en charge **app-server** et
les outils dynamiques expérimentaux, puis lancer **`:ai login`**. Le navigateur
ouvre l'authentification ChatGPT ; l'URL est aussi affichée dans la conversation
(mode manuel/SSH). **`:ai status`** affiche la connexion et **`:ai logout`** la
supprime du home Codex configuré.

Si Codex renvoie « model is not supported when using Codex with a ChatGPT
account », consulter **`:ai models`**, vérifier la version de l'exécutable
configuré (`codex --version` pour la commande par défaut) et les accès du compte
ou workspace. L'existence d'un modèle dans la documentation, ou sa présence dans
un catalogue éventuellement mis en cache, ne garantit pas l'accès effectif pour
ce client et ce compte. MailTUI transmet le modèle configuré au démarrage du fil
et du tour ; il n'en choisit pas un autre automatiquement. Le détail de l'erreur
est conservé dans la conversation, avec retour à la ligne et défilement.

MailTUI utilise par défaut un home Codex dédié sous
`$XDG_DATA_HOME/mailtui/codex` (repli `~/.local/share/mailtui/codex`). Codex gère
les jetons et leur renouvellement. Ce mode utilise le backend Codex, pas une
clé OAuth envoyée à un endpoint compatible quelconque. Les sessions modèle sont
éphémères ; le processus est arrêté à l'annulation, au timeout ou à la fermeture.

Sans section `[llm]`, ou avec **`enabled = false`**, aucun client externe ni
processus Codex n'est lancé. L'ouverture signale simplement que l'assistant est
désactivé.

## HTML, navigateur et SSH

Entrée sur un mail ouvre son lecteur ; **h** ouvre le HTML nettoyé avec les
images distantes bloquées, **H** les autorise dès l'ouverture. Ces deux touches
fonctionnent sur le mail sélectionné ou depuis son lecteur. Le serveur utilise
`127.0.0.1` et un port libre. Jetons aléatoires de 256 bits, TTL de dix minutes
par défaut, révocation à la sortie. Scripts, formulaires, styles du mail et
images distantes sont retirés avec `h`. Un bouton **« Afficher les images
distantes »** les active explicitement pour la page ; un second permet de les
masquer. Le navigateur les charge directement, avec une CSP adaptée ; scripts,
formulaires et iframes restent supprimés. Images CID PNG/JPEG/GIF/WebP authentifiées.
La démo contient un mail multipart HTML/CID dans chaque inbox.

`viewer.browser_command` est un tableau d'arguments sans shell.
`viewer.mode = "manual"` affiche l'URL ; SSH force ce comportement : elle
concerne l'hôte distant et nécessite un port-forward local explicite.
La configuration s'applique aussi avec `--demo --config ...`.

## Périmètre et limites

Lecture pimdir hors ligne, arbre multi-comptes, pagination par dossier,
recherche et viewer HTML sont fonctionnels. Les pages contiennent 200 mails
au plus, avec un budget global de 2 000 métadonnées chargées ; les requêtes
simultanées réservent ce budget. Le filtre et le tri s'appliquent aux mails
chargés. Un budget saturé est signalé, sans avancer les curseurs ni prétendre
avoir chargé d'autres mails. Le cache normal est remplacé par les hits pendant
une recherche et restauré à son retour.

Les écritures réelles passent par le producteur pimdir et Neverest. Undo,
suppression permanente, threads, keymaps
personnalisables et notifications automatiques restent à implémenter.
Pending/conflits upstream sont affichés ; leur résolution utilise Neverest. Aucun budget de
latence ni support de 100k messages n'est revendiqué à ce stade.

## Développement et Nix

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked
python3 tests/terminal_smoke.py target/debug/mailtui
```

## Installer dans NixOS

Le projet expose `packages.<system>.default` (également `mailtui`),
`nixosModules.default` et `overlays.default`. Exemple avec une entrée flake :

```nix
{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
  inputs.mailtui.url = "path:/chemin/vers/email-tui";
  # Une URL Git peut remplacer ce chemin une fois le projet publié.

  outputs = inputs@{ nixpkgs, mailtui, ... }: {
    nixosConfigurations.mon-pc = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        mailtui.nixosModules.default
        ({ pkgs, ... }: {
          programs.mailtui = {
            enable = true;
            package = mailtui.packages.${pkgs.stdenv.hostPlatform.system}.default;
            settings = {
              accounts = [{
                id = "personal";
                label = "Personal";
                email = "me@example.com";
                store = "/home/me/.local/state/neverest/personal";
              }];
            };
          };
        })
      ];
    };
  };
}
```

Configurer Carillon et Neverest séparément pour alimenter pimdir et traiter
la file producteur. Les secrets restent dans leur configuration.
Le module écrit les valeurs par défaut dans `/etc/xdg/mailtui/config.toml` ;
`~/.config/mailtui/config.toml` ou `--config` prend priorité.

Sans flake : `pkgs.callPackage /chemin/email-tui/package.nix {}` ou importer
`/chemin/email-tui/nixos-module.nix` et activer `programs.mailtui.enable`.
`default.nix` permet aussi `nix-build`.

Pour cette copie non suivie : `nix build path:.` / `nix run path:. -- --demo`.
[Résultats de validation Nix](docs/development.md).

[Architecture](docs/data-model.md) · [Confidentialité](docs/security.md).
