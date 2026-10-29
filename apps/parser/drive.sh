# Run from this directory after `kobo dev` serves localhost:8787.
# The simulator starts with an empty private shelf; this records the honest
# first-run transfer instructions rather than the removed toy story.
clean
expect No stories yet
expect On your computer
expect parser push
expect Check for stories
shot parser-library-empty
