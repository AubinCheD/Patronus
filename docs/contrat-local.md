# Contrat local : proposition de Patronus

Proposition, à discuter avec Fawkes et DragonTree (voir Fawkes, `docs/reflexions.md` §3.1). Rien de
ce qui suit n'est encore implémenté chez eux ; Patronus fonctionne seul en attendant.

## Principe

- Une demande est **une ligne JSON** envoyée sur le canal nommé de l'application destinataire,
  réservé au compte (`\\.\pipe\<application>-<SID du compte>`), ou passée en ligne de commande
  quand l'application ne tourne pas (elle démarre avec).
- Chaque demande porte la version du contrat : `{"v": 1, "demande": "...", ...}`. Un destinataire
  qui ne connaît pas une demande répond `{"v": 1, "ok": false, "erreur": "demande inconnue"}` ; rien
  n'est ignoré en silence.
- Réponse : une ligne JSON, `{"v": 1, "ok": true, ...}`.
- Pas de schéma d'adresse (`patronus://`) : n'importe quelle page web pourrait l'appeler.

## Ce que Patronus reçoit

| Demande | Champs | Effet | Ligne de commande |
|---|---|---|---|
| `lancer` | `tache` | lance la tâche (comme *▶*) ; réponse : identifiant de l'exécution | `patronus lancer <tâche>` (**existe**, sans canal : exécute dans le processus appelant) |
| `afficher_resultat` | `execution` | ouvre la boîte sur cette entrée | `patronus afficher <exécution>` |
| `nouvelle_tache` | `depot` | ouvre l'éditeur, dépôt prérempli (DragonTree : *Lancer une tâche sur ce dépôt*) | `patronus nouvelle --depot <dossier>` |
| `evenement` | `source`, `type`, `dossier`, `texte` | déclencheurs venus d'ailleurs, voir ci-dessous | — |

Événements attendus :
- **Fawkes**, motif dans la sortie d'un terminal : `{"source": "fawkes", "type": "motif", "dossier":
  "<dossier du terminal>", "terminal": "<titre>", "texte": "<ligne>"}`. Le motif est déclaré dans la
  tâche ; Fawkes a besoin de la liste des motifs à surveiller : Patronus la lui envoie
  (`surveiller_motifs`, ci-dessous) à chaque changement.
- **Fawkes ou Menagerie**, service arrêté : `{"source": "menagerie", "type": "service_arrete",
  "dossier": "...", "texte": "<nom du service>"}`.
- **Owlery**, PR ou CI : `{"source": "owlery", "type": "pr_ouverte" | "pr_commentee" | "ci_echec",
  "depot": "...", "lien": "..."}`.

## Ce que Patronus demande

| À | Demande | Champs | Usage |
|---|---|---|---|
| Fawkes | `ouvrir` | `dir`, `groupe` (« Agents »), `titre`, `claude` (bool), `resume` (id de session), `consigne` (collée sans être envoyée) | *Reprendre dans Fawkes* une session lancée par une tâche, ou action « session interactive ». Par rapport à `fawkes.exe open` décrit dans les réflexions de Fawkes, il manque `groupe` et `consigne`. |
| Fawkes | `surveiller_motifs` | liste de `{tache, dossier, motif}` | Watchers sur la sortie des terminaux. |
| DragonTree | `ouvrir` | `dir`, `diff` (fichier `.patch` à afficher) | Ouvrir le diff d'un résultat dans DragonTree. Aujourd'hui `dragontree.exe <dossier>` suffit pour le dépôt ; l'affichage d'un patch est à ajouter. |

## Sécurité

- Canal limité au compte (descripteur de sécurité du tube), pas d'accès réseau.
- Une demande ne contient jamais de secret.
- Les demandes qui agissent vers l'extérieur (pousser, commenter une PR) ne font pas partie du
  contrat : elles passent toujours par un clic dans l'application concernée.
