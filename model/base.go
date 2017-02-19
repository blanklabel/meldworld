package model

// types of messages
const (
	CLIENTJOIN    string = "client.join"
	CLIENTLEAVE   string = "client.leave"
	CLIENTERROR   string = "client.error"
	CLIENTMESSAGE string = "client.message"
	WORLDMAP      string = "worldmap"
	ENTITYACTION  string = "entity.action"
)

// Location of an object, entity, or tile
type Cords struct {
	X, Y int
}
