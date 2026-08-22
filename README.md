# text3d

Éditeur de texte en trois dimensions : chaque glyphe est un maillage extrudé et biseauté, la
caméra orbite autour du document, et l'interface d'IDE (arbre de fichiers, onglets, palettes,
diagnostics, sortie des tâches) flotte en HUD ancré à la caméra devant la scène.

## Projet

| | |
|---|---|
| cmd + O | ouvrir un projet |
| cmd + P | ouverture rapide de fichier |
| cmd + shift + O | symboles du document |
| cmd + T | symboles du projet |
| cmd + shift + F | rechercher dans le projet |
| cmd + shift + P | palette de commandes |
| cmd + shift + E | arbre de fichiers |
| ctrl + ' | panneau de sortie |
| cmd + shift + M | problèmes |

## Langage

| | |
|---|---|
| F12, cmd + clic | aller à la définition |
| shift + F12 | références |
| F2 | renommer |
| shift + option + F | formater |

## Tâches

| | |
|---|---|
| cmd + B | vérifier (cargo check / dotnet build) |
| cmd + shift + B | compiler |
| cmd + R | lancer |
| cmd + shift + T | tester |
| cmd + shift + K | clippy |
| cmd + . | arrêter la tâche |

## Onglets

| | |
|---|---|
| cmd + W | fermer l'onglet |
| ctrl + tab, ctrl + shift + tab | onglet suivant, précédent |
| cmd + 1 à cmd + 9 | aller à l'onglet n |
| ctrl + option + gauche / droite | retour, avant dans les sauts |

## Édition

| | |
|---|---|
| shift + flèches | étendre la sélection |
| cmd + option + flèches | se déplacer par mot |
| cmd + A | tout sélectionner |
| cmd + C / X / V | copier, couper, coller |
| cmd + Z, cmd + shift + Z | annuler, refaire |
| option + retour | effacer le mot à gauche |
| cmd + S, cmd + option + S | enregistrer, tout enregistrer |
| tab, ctrl + espace | ouvrir la complétion |
| cmd + gauche / droite | début ou fin de ligne |
| cmd + haut / bas | haut ou bas du document |
| cmd + Q | quitter |

## Recherche dans le fichier

| | |
|---|---|
| cmd + F | ouvrir la barre |
| entrée, shift + entrée | occurrence suivante, précédente |
| tab | champ de remplacement |
| cmd + entrée | tout remplacer |
| cmd + G | suivante sans la barre |
| échap | fermer |

## Vue

| | |
|---|---|
| option + 1 | recadrer |
| option + 2 | ondulation |
| option + 3 | grille |
| option + 4 | ombres portées |
| option + 5 | fonte suivante |
| option + 6 | biseau |
| option + 7 | relief par indentation |
| option + 8 | numéros de ligne |

## Souris

| | |
|---|---|
| clic gauche | poser le curseur |
| double clic, triple clic | sélectionner le mot, la ligne |
| shift + glisser | sélectionner à la souris |
| glisser gauche | tourner autour du texte |
| glisser droit | translater |
| molette | zoom au-dessus de l'éditeur, défilement au-dessus d'un panneau |

## Export

| | |
|---|---|
| option + E | maillage obj |
| option + shift + E | maillage glb |
| option + P | capture png |

## Aperçus

![relief](media/apercu-relief.png)
![complétion](media/apercu-completion.png)
