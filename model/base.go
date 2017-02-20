package model

// types of messages
const (
	CLIENTJOIN    string = "client.join"    //player joins
	CLIENTLEAVE   string = "client.leave"   // player leaves
	CLIENTERROR   string = "client.error"   // error messages
	CLIENTMESSAGE string = "client.message" // chat messages
	WORLDMAP      string = "worldmap"       // Product a world map
	ENTITYACTION  string = "entity.action"  // entity moves or attacks
	PLAYERINFO    string = "player.info"    // Basic info about a player
	ENTITYUPDATE  string = "entity.update"  // update entity state (HP down, new location etc)
)

// Location of an object, entity, or tile
type Cords struct {
	X, Y int
}
